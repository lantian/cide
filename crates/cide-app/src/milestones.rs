//! Milestone gates and branch verification, run by cide rather than asked of a model. (M83)
//!
//! `cide_ipc::milestones` says why milestones exist and `cide_agents::milestones` what they decide
//! about a board. This module is the part that runs programs and remembers what they said:
//!
//! * **A gate** — the active milestone's command, run in the project root after every
//!   integration and on request. When it passes, the milestone's task moves to *review* with the
//!   output as a comment and the spinner stops waking the project: the next step is the user's.
//! * **Verify** — the project's own check, run in a run's worktree before
//!   `cide_agent_integrate` merges its branch. Red refuses the merge with the output, which is the
//!   reviewer's hand-back already written. It is warmed as soon as a run sets its task to review,
//!   so the reviewer rarely waits for it.
//! * **Guards** — a branch that touches a path a gate reads is refused at integration. A goal the
//!   agent measured against it can edit is not a goal.
//!
//! None of it applies to the user's own Integrate button: these are rules about work a model
//! merges, and the person at the keyboard is who the rules answer to.
//!
//! # Threads
//!
//! Every check runs on a thread this module spawns and joins nothing on — `check::run` waits on
//! the calling thread, which is `arm`'s contract, and the only caller that blocks on a result is
//! the integrate tool's connection thread, the one thread in the process allowed to wait for work
//! it asked for (`RegistrySink::integrate` says so about the merge itself).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use cide_ipc::milestones::{DEFAULT_GATE_TIMEOUT_SECS, GateState};
use cide_ipc::{
    AgentId, CheckResult, MilestonePlan, MilestonesView, ProjectId, TaskAuthor, TaskEdit, TaskId,
    TaskStatus,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::tasks_state::TasksStores;
use crate::workspace_state::WorkspaceState;

/// What this process knows about checks, keyed by project root.
#[derive(Default)]
pub struct Checks {
    inner: Mutex<HashMap<PathBuf, RootChecks>>,
    /// Signalled whenever a verify finishes, so a caller waiting on one already in flight wakes.
    done: Condvar,
    loaded: std::sync::OnceLock<()>,
}

#[derive(Default, Clone, Serialize, Deserialize)]
struct RootChecks {
    /// The last result of each milestone's gate, by milestone id.
    gates: HashMap<String, CheckResult>,
    #[serde(skip)]
    gates_running: Vec<String>,
    /// Verify results by commit — a verdict is about a commit, so a later push re-checks.
    #[serde(skip)]
    verified: HashMap<String, CheckResult>,
    #[serde(skip)]
    verifying: Vec<String>,
    /// Verify as the board sees it: by task, with who did the work, running or last answered.
    /// Memory only — a verdict is about a commit on this machine, and the panel shows it while it
    /// is news. (M83)
    #[serde(skip)]
    task_verifies: HashMap<TaskId, (AgentId, bool, Option<CheckResult>)>,
    /// The commit the gate last ran against, per milestone, so the spinner can tell whether
    /// anything landed since.
    #[serde(default)]
    gate_heads: HashMap<String, String>,
}

/// Where gate results are kept between launches: a fact about this machine, not the project.
fn store_path() -> PathBuf {
    cide_core::persist::state_dir().join("gates.json")
}

/// What a check log is about. The two kinds never share a file.
#[derive(Clone, Copy)]
pub enum LogKind {
    Gate,
    Verify,
}

/// The full log of the latest `kind` check keyed `key` (a milestone id, a task id) in `root`.
///
/// In the profile's state directory, **not** under `.cide/`: that directory is committed, and a
/// twenty-minute test run's output is not something a repository should carry. One directory per
/// project, named from its path so it can be found by hand; `None` for a key that is not a plain
/// name, since it becomes a file name and a `../` in it would be a path out of this directory.
pub fn log_path(root: &Path, kind: LogKind, key: &str) -> Option<PathBuf> {
    if key.is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        || key.starts_with('.')
    {
        return None;
    }
    let project: String = root
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let file = match kind {
        LogKind::Gate => format!("gate-{key}.log"),
        LogKind::Verify => format!("verify-{key}.log"),
    };
    Some(
        cide_core::persist::state_dir()
            .join("check-logs")
            .join(project.trim_matches('_'))
            .join(file),
    )
}

/// How much of a log the panel is handed: the end, where the verdict is.
const LOG_READ_CAP: u64 = 1024 * 1024;

/// The panel's read of one log: its last [`LOG_READ_CAP`] bytes, or `None` when there is none.
pub fn read_log(root: &Path, kind: LogKind, key: &str) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let path = log_path(root, kind, key)?;
    let mut file = std::fs::File::open(&path).ok()?;
    let len = file.metadata().ok()?.len();
    let mut prefix = String::new();
    if len > LOG_READ_CAP {
        file.seek(SeekFrom::Start(len - LOG_READ_CAP)).ok()?;
        prefix = format!(
            "[cide] showing the last {} KiB of {} KiB — the whole file is {}\n",
            LOG_READ_CAP / 1024,
            len / 1024,
            path.display()
        );
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).ok()?;
    Some(format!("{prefix}{}", String::from_utf8_lossy(&bytes)))
}

fn log_string(root: &Path, kind: LogKind, key: &str) -> Option<String> {
    log_path(root, kind, key)
        .filter(|p| p.is_file())
        .map(|p| p.display().to_string())
}

impl Checks {
    fn with<T>(&self, root: &Path, f: impl FnOnce(&mut RootChecks) -> T) -> T {
        self.loaded.get_or_init(|| {
            let loaded: HashMap<PathBuf, RootChecks> = std::fs::read(store_path())
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default();
            if let Ok(mut inner) = self.inner.lock() {
                *inner = loaded;
            }
        });
        let mut inner = self.inner.lock().expect("checks lock");
        f(inner.entry(root.to_path_buf()).or_default())
    }

    fn save(&self) {
        let snapshot = match self.inner.lock() {
            Ok(inner) => inner.clone(),
            Err(_) => return,
        };
        match serde_json::to_vec_pretty(&snapshot) {
            Ok(bytes) => {
                if let Err(error) = cide_core::persist::write_atomic(&store_path(), &bytes) {
                    tracing::warn!(%error, "could not save the milestone gate results");
                }
            }
            Err(error) => tracing::warn!(%error, "could not encode the milestone gate results"),
        }
    }

    /// The last result of `milestone`'s gate in `root`.
    pub fn last_gate(&self, root: &Path, milestone: &str) -> Option<CheckResult> {
        self.with(root, |c| c.gates.get(milestone).cloned())
    }

    /// Whether the gate ran against a different commit than `head` (or never ran).
    pub fn gate_is_stale(&self, root: &Path, milestone: &str, head: Option<&str>) -> bool {
        self.with(root, |c| {
            c.gate_heads.get(milestone).map(String::as_str) != head
                || !c.gates.contains_key(milestone)
        })
    }
}

fn checks(app: &AppHandle) -> Option<tauri::State<'_, Arc<Checks>>> {
    app.try_state::<Arc<Checks>>()
}

fn root_of(app: &AppHandle, project: ProjectId) -> Option<PathBuf> {
    let workspace = app.try_state::<WorkspaceState>()?;
    crate::tasks_state::project_root(&workspace, project).ok()
}

/// Everything the Milestones tab draws.
pub fn view(app: &AppHandle, project: ProjectId) -> Option<MilestonesView> {
    let root = root_of(app, project)?;
    let plan = cide_agents::config::load_milestones(&root);
    let checks = checks(app)?;
    let (gates, running) = checks.with(&root, |c| (c.gates.clone(), c.gates_running.clone()));
    let rows = app
        .try_state::<Arc<TasksStores>>()
        .and_then(|stores| stores.get(project))
        .map(|store| store.list())
        .unwrap_or_default();
    let accepted = plan
        .items
        .iter()
        .filter(|m| {
            m.task.as_ref().is_some_and(|t| {
                rows.iter()
                    .any(|r| &r.id == t && r.status == TaskStatus::Done)
            })
        })
        .map(|m| m.id.clone())
        .collect();
    let tasks = plan
        .items
        .iter()
        .map(|m| cide_ipc::MilestoneTasks {
            milestone: m.id.clone(),
            tasks: m
                .task
                .as_ref()
                .map(|goal| {
                    cide_agents::milestones::tree_under(&rows, goal)
                        .into_iter()
                        .map(|(depth, r)| cide_ipc::MilestoneTask {
                            id: r.id.clone(),
                            title: r.title.clone(),
                            status: r.status,
                            agent: r.agent.clone(),
                            depth,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect();
    let verifies = checks.with(&root, |c| {
        let mut all: Vec<cide_ipc::VerifyState> = c
            .task_verifies
            .iter()
            .map(|(task, (agent, running, last))| cide_ipc::VerifyState {
                task: task.clone(),
                agent: agent.clone(),
                running: *running,
                last: last.clone(),
                log: log_string(&root, LogKind::Verify, task.as_str()),
            })
            .collect();
        all.sort_by_key(|v| {
            std::cmp::Reverse(v.last.as_ref().map_or(u64::MAX, |r| r.started_unix_ms))
        });
        all
    });
    let proposals = crate::proposals::list(app, &root);
    Some(MilestonesView {
        project,
        tasks,
        verifies,
        proposals,
        gates: plan
            .items
            .iter()
            .map(|m| GateState {
                milestone: m.id.clone(),
                last: gates.get(&m.id).cloned(),
                running: running.contains(&m.id),
                log: log_string(&root, LogKind::Gate, &m.id),
            })
            .collect(),
        plan,
        accepted,
    })
}

/// Run the active milestone's gate in the background. Does nothing when there is no milestone,
/// and answers the gate already in flight rather than starting a second one.
pub fn run_gate(app: &AppHandle, project: ProjectId) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("cide-gate".into())
        .spawn(move || {
            run_gate_now(&app, project);
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not start a thread for the milestone gate");
    }
}

/// Run the active gate and wait for it — the spinner's road, which is already on its own thread
/// and must plan from a fresh answer rather than from one started a moment ago.
pub fn run_gate_now(app: &AppHandle, project: ProjectId) -> Option<CheckResult> {
    let root = root_of(app, project)?;
    let plan = cide_agents::config::load_milestones(&root);
    let milestone = plan.current()?.clone();
    if milestone.gate.trim().is_empty() {
        return None;
    }
    let checks = checks(app).map(|s| Arc::clone(&s))?;
    // One lock for the test and the claim, or two callers both see "not running" and both run.
    let claimed = checks.with(&root, |c| {
        if c.gates_running.contains(&milestone.id) {
            false
        } else {
            c.gates_running.push(milestone.id.clone());
            true
        }
    });
    if !claimed {
        return checks.last_gate(&root, &milestone.id);
    }
    crate::emit::milestones_changed(app, project);
    let timeout = Duration::from_secs(milestone.timeout_secs.unwrap_or(DEFAULT_GATE_TIMEOUT_SECS));
    let log = log_path(&root, LogKind::Gate, &milestone.id);
    let result = cide_core::check::run_logged(&root, &milestone.gate, timeout, log.as_deref());
    tracing::info!(
        %project, milestone = %milestone.id, passed = result.passed,
        exit = ?result.exit_code, ms = result.duration_ms,
        "milestone gate ran"
    );
    checks.with(&root, |c| {
        c.gates_running.retain(|m| m != &milestone.id);
        if let Some(head) = &result.head {
            c.gate_heads.insert(milestone.id.clone(), head.clone());
        }
        c.gates.insert(milestone.id.clone(), result.clone());
    });
    checks.save();
    if result.passed {
        announce_met(app, project, &milestone, &result);
    }
    crate::emit::milestones_changed(app, project);
    Some(result)
}

/// A gate passed: the milestone's task goes to review with the evidence, once.
fn announce_met(
    app: &AppHandle,
    project: ProjectId,
    milestone: &cide_ipc::Milestone,
    result: &CheckResult,
) {
    let Some(task) = milestone.task.as_ref() else {
        return;
    };
    let Some(store) = app
        .try_state::<Arc<TasksStores>>()
        .and_then(|stores| stores.get(project))
    else {
        return;
    };
    let Some(current) = store.get(task) else {
        return;
    };
    if matches!(current.status, TaskStatus::Review | TaskStatus::Done) {
        return;
    }
    let text = format!(
        "**cide: the gate for milestone `{}` passes.** It stays in review until you accept it in \
         the Milestones tab of the Tasks panel; until then cide does not wake this project to plan more.\n\n\
         `{}` — exit 0 in {}s{}\n\n```\n{}\n```",
        milestone.id,
        result.command,
        result.duration_ms / 1000,
        result
            .head
            .as_deref()
            .map(|h| format!(" at {}", &h[..h.len().min(10)]))
            .unwrap_or_default(),
        last_lines(&result.tail, 20)
    );
    let author = TaskAuthor::Orchestrator;
    let _ = store.edit(task, TaskEdit::Comment { text }, author.clone());
    let _ = store.edit(
        task,
        TaskEdit::SetStatus {
            status: TaskStatus::Review,
        },
        author,
    );
    crate::tasks_state::broadcast(app, project, &store);
}

/// The user accepts the active milestone: its task is done and the next one becomes active.
pub fn accept(app: &AppHandle, project: ProjectId) -> Result<(), String> {
    let root = root_of(app, project).ok_or("that project is not open")?;
    let mut plan = cide_agents::config::load_milestones(&root);
    let current = plan
        .current()
        .cloned()
        .ok_or("this project has no milestones")?;
    if let Some(task) = current.task.as_ref()
        && let Some(store) = app
            .try_state::<Arc<TasksStores>>()
            .and_then(|stores| stores.get(project))
    {
        store
            .edit(
                task,
                TaskEdit::SetStatus {
                    status: TaskStatus::Done,
                },
                TaskAuthor::User,
            )
            .map_err(|e| e.to_string())?;
        crate::tasks_state::broadcast(app, project, &store);
    }
    plan.active = plan.next_after(&current.id).map(|m| m.id.clone());
    if plan.active.is_none() {
        // The last one: keep it named so the plan still says where the project ended.
        plan.active = Some(current.id.clone());
    }
    cide_agents::config::write_milestones(&root, &plan).map_err(|e| e.to_string())?;
    crate::emit::milestones_changed(app, project);
    Ok(())
}

/// Replace the plan — the Milestones tab's write, and `cide_milestones define`'s.
///
/// A milestone with no task gets one here, before the config is written, because every rule that
/// keys off the active milestone (placement, the dispatch gate, the spinner's facts) reads its
/// **task**: a milestone the user added in the panel without one would be a goal nothing is
/// measured against. `plan` is updated in place with the ids it was given.
pub fn set_plan(
    app: &AppHandle,
    project: ProjectId,
    plan: &mut MilestonePlan,
    author: TaskAuthor,
) -> Result<(), String> {
    let root = root_of(app, project).ok_or("that project is not open")?;
    let mut ids: Vec<String> = Vec::new();
    for m in &mut plan.items {
        m.id = m.id.trim().to_string();
        if m.id.is_empty() {
            return Err("every milestone needs an id".into());
        }
        if ids.contains(&m.id) {
            return Err(format!("two milestones are called `{}`", m.id));
        }
        ids.push(m.id.clone());
    }
    if plan.items.iter().any(|m| m.task.is_none()) {
        let stores = app
            .try_state::<Arc<TasksStores>>()
            .ok_or("cide is shutting down")?;
        let store = stores.ensure(project, &root);
        for milestone in plan.items.iter_mut().filter(|m| m.task.is_none()) {
            let title = if milestone.title.trim().is_empty() {
                milestone.id.clone()
            } else {
                milestone.title.trim().to_string()
            };
            let req = cide_ipc::TaskNew {
                project,
                title: format!("Milestone `{}` — {title}", milestone.id),
                body: Some(format!(
                    "A goal this project works towards. It is met when this passes, run by cide \
                     in the project root:\n\n```sh\n{}\n```\n\nWork towards it is a subtask of \
                     this task. When the gate passes cide moves this task to review, and the user \
                     accepts it in the Milestones tab of the Tasks panel.",
                    milestone.gate
                )),
                agent: None,
                status: None,
                change: None,
                links: None,
                attachments: None,
            };
            let task = store
                .create(&req, author.clone())
                .map_err(|e| e.to_string())?;
            milestone.task = Some(task.id);
        }
        crate::tasks_state::broadcast(app, project, &store);
    }
    cide_agents::config::write_milestones(&root, plan).map_err(|e| e.to_string())?;
    crate::emit::milestones_changed(app, project);
    Ok(())
}

/// The checks a branch must pass before a model may merge it: guards, a clean worktree, and the
/// project's verify command. `Ok` when the project asks for none of them.
///
/// Blocks for as long as verify takes — see the module header for why that is allowed here.
pub fn before_integrate(
    app: &AppHandle,
    project: ProjectId,
    root: &Path,
    agent: &AgentId,
    task: Option<&TaskId>,
) -> Result<(), String> {
    let plan = cide_agents::config::load_milestones(root);
    let name = cide_agents::checkout_name(agent, task);
    let branch = format!("cide/{name}");

    if !plan.guard_paths.is_empty() {
        let touched = changed_paths(root, &branch)?;
        let guarded: Vec<&String> = touched.iter().filter(|p| plan.guards(p)).collect();
        if !guarded.is_empty() {
            return Err(format!(
                "not merged: {branch} changes {} — a file a milestone gate reads. Work may not \
                 move the goal it is measured against; changing a gate is the user's call, made \
                 in the Milestones tab of the Tasks panel or by hand. Hand the task back to drop that change, or propose the \
                 gate change with cide_milestones.",
                guarded
                    .iter()
                    .map(|p| format!("`{p}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    if plan.verify.trim().is_empty() {
        return Ok(());
    }
    let worktree = cide_git::worktree::path_of(root, &name);
    if !worktree.is_dir() {
        // Nothing to run it in, and nothing we could merge that the run did not commit: say so
        // rather than merging unverified.
        return Err(format!(
            "not merged: the project's verify command (`{}`) runs in the run's checkout, and \
             .cide/worktrees/{name} is gone. Dispatch the role on the task again to recreate it.",
            plan.verify
        ));
    }
    let checks = checks(app)
        .map(|s| Arc::clone(&s))
        .ok_or("cide is shutting down")?;
    let dirty = dirty_paths(&worktree);
    if !dirty.is_empty() {
        let why = format!(
            "not merged: .cide/worktrees/{name} has changes that were never committed ({}). \
             What is merged is the branch, so they would be lost, and verify would be checking \
             something other than what lands. Hand the task back to commit or discard them.",
            dirty.iter().take(8).cloned().collect::<Vec<_>>().join(", ")
        );
        // Drawn on the task like a failed verify, because to the person watching it is one: the
        // work was checked before a merge and did not get through. (M83)
        if let Some(task) = task {
            let refused = CheckResult {
                command: plan.verify.clone(),
                passed: false,
                exit_code: None,
                timed_out: false,
                tail: why.clone(),
                started_unix_ms: now_ms(),
                duration_ms: 0,
                head: None,
            };
            note_verify(
                app,
                project,
                &checks,
                root,
                agent,
                task,
                false,
                Some(refused),
            );
        }
        return Err(why);
    }
    let result = verify_task(
        app,
        project,
        &checks,
        root,
        &worktree,
        &plan.verify,
        agent,
        task,
    );
    if result.passed {
        Ok(())
    } else {
        let full = task
            .and_then(|t| log_string(root, LogKind::Verify, t.as_str()))
            .map(|p| format!("\nThe whole output is in {p}."))
            .unwrap_or_default();
        Err(format!(
            "not merged: the project's verify command failed on {branch}{}. Hand it back with \
             this output as the feedback.{full}\n\n`{}` — {}\n```\n{}\n```",
            result
                .head
                .as_deref()
                .map(|h| format!(" at {}", &h[..h.len().min(10)]))
                .unwrap_or_default(),
            result.command,
            match (result.timed_out, result.exit_code) {
                (true, _) => "timed out".to_string(),
                (false, Some(code)) => format!("exit {code}"),
                (false, None) => "killed".to_string(),
            },
            last_lines(&result.tail, 30)
        ))
    }
}

/// Verify `worktree`'s commit, once per commit: a result already known is answered, one in flight
/// is waited for, and otherwise it runs here.
fn verify(
    checks: &Checks,
    root: &Path,
    worktree: &Path,
    command: &str,
    log: Option<&Path>,
) -> CheckResult {
    let head = cide_core::check::head_of(worktree).unwrap_or_default();
    loop {
        let known = checks.with(root, |c| {
            if let Some(done) = c.verified.get(&head) {
                return Some(Some(done.clone()));
            }
            if c.verifying.contains(&head) {
                return Some(None);
            }
            c.verifying.push(head.clone());
            None
        });
        match known {
            Some(Some(done)) if done.command == command => return done,
            Some(Some(_)) => {
                // Verified with a different command — the setting changed since. Run again.
                checks.with(root, |c| {
                    c.verified.remove(&head);
                });
                continue;
            }
            Some(None) => {
                let guard = checks.inner.lock().expect("checks lock");
                let _ = checks.done.wait_timeout(guard, Duration::from_secs(5));
                continue;
            }
            None => break,
        }
    }
    let result = cide_core::check::run_logged(
        worktree,
        command,
        Duration::from_secs(DEFAULT_GATE_TIMEOUT_SECS),
        log,
    );
    checks.with(root, |c| {
        c.verifying.retain(|h| h != &head);
        if !head.is_empty() {
            c.verified.insert(head.clone(), result.clone());
        }
    });
    checks.done.notify_all();
    result
}

/// Start verify for a run's branch in the background, so it is ready when a reviewer merges.
pub fn prewarm(app: &AppHandle, project: ProjectId, agent: AgentId, task: TaskId) {
    let Some(root) = root_of(app, project) else {
        return;
    };
    let plan = cide_agents::config::load_milestones(&root);
    if plan.verify.trim().is_empty() {
        return;
    }
    let worktree =
        cide_git::worktree::path_of(&root, &cide_agents::checkout_name(&agent, Some(&task)));
    if !worktree.is_dir() || !dirty_paths(&worktree).is_empty() {
        return;
    }
    let Some(checks) = checks(app).map(|s| Arc::clone(&s)) else {
        return;
    };
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("cide-verify".into())
        .spawn(move || {
            let result = verify_task(
                &app,
                project,
                &checks,
                &root,
                &worktree,
                &plan.verify,
                &agent,
                Some(&task),
            );
            tracing::info!(%agent, %task, passed = result.passed, "verify ran on review");
        });
}

/// [`verify`], with the board told it is running and what it answered. (M83)
#[allow(clippy::too_many_arguments)]
fn verify_task(
    app: &AppHandle,
    project: ProjectId,
    checks: &Checks,
    root: &Path,
    worktree: &Path,
    command: &str,
    agent: &AgentId,
    task: Option<&TaskId>,
) -> CheckResult {
    if let Some(task) = task {
        let previous = checks.with(root, |c| {
            c.task_verifies.get(task).and_then(|v| v.2.clone())
        });
        note_verify(app, project, checks, root, agent, task, true, previous);
    }
    let log = task.and_then(|t| log_path(root, LogKind::Verify, t.as_str()));
    let result = verify(checks, root, worktree, command, log.as_deref());
    if let Some(task) = task {
        note_verify(
            app,
            project,
            checks,
            root,
            agent,
            task,
            false,
            Some(result.clone()),
        );
    }
    result
}

/// Record one task's verify state and tell the windows.
#[allow(clippy::too_many_arguments)]
fn note_verify(
    app: &AppHandle,
    project: ProjectId,
    checks: &Checks,
    root: &Path,
    agent: &AgentId,
    task: &TaskId,
    running: bool,
    last: Option<CheckResult>,
) {
    checks.with(root, |c| {
        c.task_verifies
            .insert(task.clone(), (agent.clone(), running, last));
    });
    crate::emit::milestones_changed(app, project);
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Files the branch changes relative to where it forked from `HEAD`.
fn changed_paths(root: &Path, branch: &str) -> Result<Vec<String>, String> {
    let out = git(root, &["diff", "--name-only", &format!("HEAD...{branch}")])
        .map_err(|e| format!("could not read what {branch} changes: {e}"))?;
    Ok(out
        .lines()
        .map(str::to_string)
        .filter(|l| !l.is_empty())
        .collect())
}

/// Tracked changes and untracked, unignored files in a checkout.
fn dirty_paths(dir: &Path) -> Vec<String> {
    git(dir, &["status", "--porcelain", "--untracked-files=normal"])
        .map(|out| {
            out.lines()
                .filter_map(|l| l.get(3..))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args)
        .current_dir(dir)
        .stdin(std::process::Stdio::null());
    cide_core::child_env::prepare_command(&mut cmd);
    cide_core::child_env::arm(&mut cmd);
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A log key becomes a file name, so anything but a plain name is refused rather than
    /// joined — a task id is `t-12`, a milestone id `slice`, and neither has a `/` in it.
    #[test]
    fn a_log_key_cannot_leave_the_log_directory() {
        let root = Path::new("/home/me/work/game");
        let good = log_path(root, LogKind::Gate, "slice").expect("a plain name");
        assert!(
            good.ends_with("home_me_work_game/gate-slice.log"),
            "{}",
            good.display()
        );
        assert!(log_path(root, LogKind::Verify, "t-12").is_some());
        for bad in ["", "../x", "a/b", ".hidden", "..", "x y"] {
            assert!(log_path(root, LogKind::Gate, bad).is_none(), "{bad:?}");
        }
    }
}
