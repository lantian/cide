//! OpenSpec sessions: Propose, Explore and Apply as runs of their own, with no task.
//!
//! # Why runs and not the tabs `cide_session_open` opens
//!
//! The user's ask was a tab per session that can be **closed while the work goes on**, with a
//! button that opens it again. A tab `claude_tab::open` builds is `ephemeral` — closing it kills
//! its child (`cmd::project::tab_close`) — so that road cannot keep the promise. The agent
//! registry can: a run's child is owned by the registry, the tab the panel opens onto it is a
//! mirror (`tab_new_run`), and closing a mirror detaches a sink. It is exactly the road the MR
//! reviewer took (M85, `cmd::gitlab::gitlab_review_launch`), and this module is shaped after it:
//! the launcher composes everything the run needs, enqueues it, and answers the run id; the panel
//! follows the run through `cide://agents-changed` and opens the tab.
//!
//! # Why no task
//!
//! Until these existed every OpenSpec gesture went through the tracker — *Start work* made a
//! task, and the task card dispatched it. A project that drives its work from OpenSpec alone got
//! a board of tasks nobody asked for, and agents told to use the board went on filing more. A
//! session here is found again by its **change** ([`RunPurpose::Spec`]), which is persisted with
//! the run, and its branch is read from git ([`spec_checkouts`]).
//!
//! # Where Apply stands
//!
//! In the project root, unless `.cide/config.json`'s `openspec.applyInWorktree` is on — then in
//! `.cide/worktrees/spec-<change>` on branch `cide/spec-<change>`, which Publish pushes and
//! Integrate merges. Propose and Explore always stand in the root: a proposal written in a
//! worktree would be invisible to the panel, which reads the root, until it was merged.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cide_core::CoreError;
use cide_ipc::{
    ChangeName, Harness, ProjectId, RunId, RunState, SpecCheckout, SpecIntegrated, SpecLauncher,
    SpecOp, SpecPublished, SpecRunRow, SpecSessionStart, SpecSettings,
};
use tauri::{AppHandle, Manager, State};

use super::spec::{apply_invocation, blocking, follow_file, for_harness, resolve_command};
use crate::agents::{AgentRegistry, DispatchSpec, RunPurpose, SpecPurpose};
use crate::tasks_state;
use crate::workspace_state::WorkspaceState;

type Result<T> = std::result::Result<T, CoreError>;

/// The role id a main-model session runs under. Never a file: `LoadedAgent::synthetic` builds
/// it, and the purpose — not this id — is what makes a run an OpenSpec session.
const SPEC_AGENT: &str = "openspec";

/// `spec-<change>`: the worktree an applied change lives in, and (under `cide/`) its branch.
pub(crate) fn checkout_for(change: &str) -> String {
    format!("spec-{change}")
}

/// A change name is interpolated into a worktree path, a branch and a line typed at a TUI, so
/// it is checked here, once, with OpenSpec's own kebab rule.
fn checked_change(change: &str) -> Result<&str> {
    if cide_spec::claude::valid_name(change) && checkout_for(change).len() <= 64 {
        Ok(change)
    } else {
        Err(CoreError::Io(format!(
            "`{change}` is not a change name cide can work in: lowercase letters, digits and \
             dashes, at most 59 characters"
        )))
    }
}

/// The harnesses a session can run on, and why not where it cannot — the MR reviewer's list,
/// for the same dialog.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_harnesses() -> Result<Vec<cide_ipc::gitlab::GitLabReviewHarness>> {
    crate::cmd::gitlab::gitlab_review_harnesses()
        .await
        .map_err(CoreError::Io)
}

/// `.cide/config.json`'s `openspec` key.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_settings_get(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
) -> Result<SpecSettings> {
    let root = tasks_state::project_root(&state, project)?;
    blocking(move || Ok(cide_agents::config::load_spec_settings(&root))).await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn spec_settings_set(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    settings: SpecSettings,
) -> Result<SpecSettings> {
    let root = tasks_state::project_root(&state, project)?;
    blocking(move || {
        cide_agents::config::write_spec_settings(&root, &settings)?;
        Ok(cide_agents::config::load_spec_settings(&root))
    })
    .await
}

/// Start an OpenSpec session. Answers the run at once; the queue decides when it starts, and
/// the panel opens its tab once it has a child.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_session_start(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    agents: State<'_, Arc<AgentRegistry>>,
    project: ProjectId,
    request: SpecSessionStart,
) -> Result<RunId> {
    let root = tasks_state::project_root(&state, project)?;
    let registry = Arc::clone(&agents);

    blocking(move || {
        start_session(
            &app,
            &registry,
            &root,
            project,
            request,
            cide_ipc::TaskAuthor::User,
        )
    })
    .await
}

/// Both gestures enter here. Status synchronization writes straight to the store and broadcasts
/// once, rather than passing through task_triggers and reflecting into a second Apply.
fn start_session(
    app: &AppHandle,
    registry: &Arc<AgentRegistry>,
    root: &Path,
    project: ProjectId,
    request: SpecSessionStart,
    author: cide_ipc::TaskAuthor,
) -> Result<RunId> {
    let change = (request.op == SpecOp::Apply)
        .then(|| request.change.clone())
        .flatten();
    let held = change
        .as_ref()
        .and_then(|change| registry.change_holding(project, change.as_str()));
    // Plan before changing statuses: a missing change or workflow must leave the board alone.
    let spec = if held.is_none() {
        let spec = plan_session(root, project, request.clone())?;
        // Planning Apply can create a worktree after spec_checkouts cached an empty list.
        // File events inside openspec invalidate progress, but not that git metadata. Publish
        // a fresh snapshot now so both the sidebar and change tab discover the checkout.
        if spec.checkout.is_some()
            && let Some(boards) = app.try_state::<Arc<crate::spec_state::SpecBoards>>()
        {
            boards.mark_changed(app, project);
        }
        Some(spec)
    } else {
        None
    };
    if let Some(change) = change.as_ref() {
        checked_change(change.as_str())?;
        if !root.join("openspec/changes").join(change.as_str()).is_dir() {
            return Err(CoreError::Io(format!(
                "there is no change `{change}` in openspec/changes/"
            )));
        }
        if cide_agents::config::load_tracker(root)
            && let Some(stores) = app.try_state::<Arc<crate::tasks_state::TasksStores>>()
        {
            let store = stores.ensure(project, root);
            if !matches!(store.board(), cide_ipc::TaskBoard::Unreadable { .. }) {
                if store.list().iter().any(|task| {
                    task.change.as_ref() == Some(change)
                        && task.status == cide_ipc::TaskStatus::Todo
                }) {
                    store.mark_change_doing(change, author)?;
                    crate::tasks_state::broadcast(app, project, &store);
                }
            } else {
                return Err(CoreError::Io(
                    "The task board is unreadable; repair it before starting Apply".into(),
                ));
            }
        }
    }
    let run = if let Some(held) = held {
        continue_apply(app, registry, root, project, held, &request)?
    } else {
        match registry.enqueue_unique(spec.expect("planned without a holder")) {
            Ok(run) => run,
            Err(held) if change.is_some() => {
                continue_apply(app, registry, root, project, held, &request)?
            }
            Err(held) => {
                return Err(CoreError::Io(format!(
                    "run {} already holds this work",
                    held.run
                )));
            }
        }
    };
    registry.mark_changed(app, project);
    registry.pump(app);
    Ok(run)
}

fn continue_apply(
    app: &AppHandle,
    registry: &Arc<AgentRegistry>,
    root: &Path,
    project: ProjectId,
    held: crate::agents::HeldPair,
    request: &SpecSessionStart,
) -> Result<RunId> {
    // Do not type into a busy run or queue another turn on every status echo. Idle is the
    // deliberate continuation; a paused run remains under the user's brake.
    if held.state == RunState::Idle
        && let Some(scope) = registry
            .run_scopes_for(project)
            .into_iter()
            .find(|scope| scope.run == held.run)
    {
        let change = request.change.as_ref().expect("Apply has a change");
        // A live conversation may predate the skill, and its worktree may not contain it.
        // Reading an absolute instruction file works in both cases and on every harness.
        let file = (scope.harness == Harness::Codex)
            .then(|| cide_spec::claude::codex_file(root, "apply-change"))
            .flatten()
            .or_else(|| cide_spec::claude::file(root, "apply-change"));
        let line = file
            .map(|file| {
                format!(
                    "Follow the OpenSpec apply instructions in `{}` for {change}.",
                    root.join(file).display()
                )
            })
            .unwrap_or_else(|| {
                format!(
                    "Continue implementing OpenSpec change {change} using its apply instructions."
                )
            });
        if scope.cwd != root
            && let Some(name) = scope.cwd.file_name().and_then(|name| name.to_str())
            && scope.cwd == cide_git::worktree::path_of(root, name)
        {
            carry_change(root, &scope.cwd, change.as_str(), &carried_dir(root, name))?;
            if let Some(boards) = app.try_state::<Arc<crate::spec_state::SpecBoards>>() {
                boards.mark_changed(app, project);
            }
        }
        let line = match request
            .text
            .as_deref()
            .map(super::agents::one_line)
            .filter(|text| !text.is_empty())
        {
            Some(text) => format!("{line} {text}"),
            None => line,
        };
        registry.follow_up(app, project, held.run, &line)?;
    }
    Ok(held.run)
}

/// A Doing gesture opts into OpenSpec even in projects with ordinary auto-dispatch disabled.
pub(crate) fn apply_task(
    app: &AppHandle,
    project: ProjectId,
    task: &cide_ipc::Task,
    author: cide_ipc::TaskAuthor,
) -> Result<RunId> {
    let state = app.state::<WorkspaceState>();
    let root = tasks_state::project_root(&state, project)?;
    let registry = app.state::<Arc<AgentRegistry>>();
    let stores = app.state::<Arc<crate::tasks_state::TasksStores>>();
    let store = stores.ensure(project, &root);
    if let Some(why) = super::agents::task_refusal(&root, &store, task) {
        return Err(CoreError::Io(why));
    }
    let launcher = match task.agent.clone() {
        Some(agent) => SpecLauncher::Role { agent },
        None => SpecLauncher::Harness {
            harness: state.with(|ws| {
                cide_core::harness_settings::effective(ws, Some(project))
                    .console_harness
                    .harness()
            }),
        },
    };
    start_session(
        app,
        &registry,
        &root,
        project,
        SpecSessionStart {
            op: SpecOp::Apply,
            change: task.change.clone(),
            text: None,
            launcher,
        },
        author,
    )
}

/// Everything a session needs before it is queued: its directory made, its lines composed.
fn plan_session(
    root: &Path,
    project: ProjectId,
    request: SpecSessionStart,
) -> Result<DispatchSpec> {
    let loaded = cide_agents::load_project(root);
    let text = request
        .text
        .as_deref()
        .map(crate::cmd::agents::one_line)
        .unwrap_or_default();

    // Who runs it, and on what.
    let (agent, label, harness) = match &request.launcher {
        SpecLauncher::Harness { harness } => (
            cide_ipc::AgentId(SPEC_AGENT.into()),
            format!("OpenSpec · {}", harness_label(*harness)),
            *harness,
        ),
        SpecLauncher::Role { agent } => {
            let role = loaded
                .get(agent)
                .ok_or_else(|| CoreError::Io(format!("no role named `{agent}` in this project")))?;
            (agent.clone(), role.def.label.clone(), role.def.harness)
        }
    };

    // Where it stands, and the line that starts it.
    let (cwd, checkout, change, prompt) = match request.op {
        SpecOp::Propose | SpecOp::Explore => {
            let command = if request.op == SpecOp::Propose {
                "propose"
            } else {
                "explore"
            };
            let invocation = resolve_command(root, command, harness == Harness::Codex)?;
            let line = if text.is_empty() {
                invocation
            } else {
                format!("{invocation} {text}")
            };
            (
                root.to_path_buf(),
                None,
                None,
                for_harness(harness, root, line),
            )
        }
        SpecOp::Merge => {
            let change = request
                .change
                .as_ref()
                .map(|change| change.0.clone())
                .ok_or_else(|| CoreError::Io("Merge needs the change to merge".into()))?;
            checked_change(&change)?;
            let branch = format!(
                "{}/{}",
                cide_git::worktree::BRANCH_PREFIX,
                checkout_for(&change)
            );
            let line = format!(
                "Merge the branch {branch} (the OpenSpec change {change}) into the branch this \
                 project has checked out, and resolve the conflicts. cide's Integrate could not \
                 merge it by itself."
            );
            let line = if text.is_empty() {
                line
            } else {
                format!("{line} {text}")
            };
            (root.to_path_buf(), None, Some(change), line)
        }
        SpecOp::Apply => {
            let change = request
                .change
                .as_ref()
                .map(|change| change.0.clone())
                .ok_or_else(|| CoreError::Io("Apply needs the change it implements".into()))?;
            checked_change(&change)?;
            if !root.join("openspec/changes").join(&change).is_dir() {
                return Err(CoreError::Io(format!(
                    "there is no change `{change}` in openspec/changes/ — it may have been archived"
                )));
            }
            let settings = cide_agents::config::load_spec_settings(root);
            let (cwd, checkout) = if settings.apply_in_worktree {
                let name = checkout_for(&change);
                let tree = cide_git::worktree::ensure(root, &name)
                    .map_err(|error| CoreError::Io(error.to_string()))?;
                carry_change(root, &tree.path, &change, &carried_dir(root, &name))?;
                (tree.path, Some(name))
            } else {
                (root.to_path_buf(), None)
            };
            // The slash form only where the session will find the skill: a claude standing in a
            // worktree whose checkout lacks it (the skills were never committed) would answer
            // `Unknown command`, so it is handed the root's file instead, as codex always is.
            //
            // Codex has its own copy under `.agents/skills/`, invoked `$openspec-apply-change`, and
            // gets it on the same terms: only where the session stands next to it. Codex finds a
            // project's skills from its working directory, and a worktree whose checkout lacks
            // `.agents/` (never committed) is not guaranteed to reach the root's — so it falls
            // back to the root's file like claude does, rather than typing a `$` line into a
            // conversation that may not have it.
            let invocation = apply_invocation(&cwd, None).or_else(|| apply_invocation(root, None));
            let slash_works = harness == Harness::Claude && apply_invocation(&cwd, None).is_some();
            let dollar = (harness == Harness::Codex)
                .then(|| cide_spec::claude::codex_line(&cwd, "apply-change"))
                .flatten();
            let line = match (dollar, invocation) {
                (Some(dollar), _) => format!("{dollar} {change}"),
                (None, Some(invocation)) => {
                    let line = format!("{invocation} {change}");
                    if slash_works {
                        line
                    } else {
                        // The root's file, never the root's `$` line (see above).
                        follow_file(root, line)
                    }
                }
                (None, None) => format!(
                    "Implement the OpenSpec change {change} as your instructions describe, \
                     starting with its apply instructions."
                ),
            };
            let line = if text.is_empty() {
                line
            } else {
                format!("{line} {text}")
            };
            (cwd, checkout, Some(change), line)
        }
    };

    let brief = cide_agents::harness::spec_session_brief(
        request.op,
        change.as_deref(),
        cide_spec::discover::find().ok().as_deref(),
        // The line this session's own CLI answers to — codex's `$` spelling for codex, which has
        // no use for Claude Code's.
        match harness {
            Harness::Codex => cide_spec::claude::codex_line(root, "apply-change"),
            _ => cide_spec::claude::line(root, "apply-change"),
        }
        .as_deref(),
        checkout.is_some(),
    );
    Ok(DispatchSpec {
        project,
        agent,
        agent_label: label,
        harness,
        task: None,
        task_title: None,
        change: None,
        prompt,
        // Several changes may be applied at once; the project's own cap still holds them all.
        agent_limit: 4,
        project_limit: loaded.config.agents.max_concurrent,
        // The admission gate's "two runs never share a checkout", for an Apply in a worktree.
        checkout: checkout.clone(),
        // A session reports to nobody's conversation: its tab is where it is watched, and the
        // panel's chip is where its state is read.
        notify: cide_ipc::RunNotify::Silent,
        purpose: RunPurpose::Spec(SpecPurpose {
            op: request.op,
            task: None,
            suppress_reveal: false,
            change,
            text,
            cwd,
            checkout,
            launcher: request.launcher,
            brief,
            dismissed: false,
        }),
        // Resolved at the fork, like a review's.
        pool: Vec::new(),
    })
}

/// The same real workflow as a standalone proposal, with access to the task it will link.
pub(super) fn plan_task_proposal(
    root: &Path,
    project: ProjectId,
    task: &cide_ipc::Task,
    harness: Harness,
) -> Result<DispatchSpec> {
    let mut spec = plan_session(
        root,
        project,
        SpecSessionStart {
            op: SpecOp::Propose,
            change: None,
            text: Some(format!("{} {}", task.title, task.body)),
            launcher: SpecLauncher::Harness { harness },
        },
    )?;
    // Tool spellings belong to the harness. A prose instruction naming Claude's tool under
    // opencode used to leave an agent hunting for a function it had never been served.
    let tools = cide_agents::harness::for_kind(harness)
        .ok_or_else(|| CoreError::Io("This harness cannot run an OpenSpec proposal".into()))?;
    spec.prompt.push_str(&format!(
        " (This proposal is for cide task {id}. Read it with {get}, including its attachments. \
         When the change exists, call {update} with task \"{id}\" and change set to the new \
         change's folder name under openspec/changes/, then leave one comment with {comment} \
         saying what you proposed. Do not implement the change or change the task's status.)",
        id = task.id,
        get = tools.tool_name("cide_task_get"),
        update = tools.tool_name("cide_task_update"),
        comment = tools.tool_name("cide_task_comment"),
    ));
    if let RunPurpose::Spec(purpose) = &mut spec.purpose {
        purpose.task = Some(task.id.clone());
        purpose.brief = format!(
            "Write an OpenSpec proposal for cide task {} using the installed propose workflow. \
             Work in the project root and leave the proposal uncommitted for the user to review. \
             Use the task tools to read the task and link the resulting change back to it. \
             Do not create tasks, implement the change, change task status, or archive it.",
            task.id
        );
    }
    Ok(spec)
}

fn harness_label(harness: Harness) -> &'static str {
    match harness {
        Harness::Claude => "Claude",
        Harness::Codex => "Codex",
        Harness::Opencode => "opencode",
        Harness::Qwen => "Qwen",
        Harness::Mimo => "MiMo",
    }
}

/// Copy `openspec/changes/<change>/` into a fresh worktree when it is not there.
///
/// A worktree is cut from `HEAD`, and a proposal is usually **uncommitted** when Apply is
/// pressed — it was written a minute ago in the root. Without this the session would stand in a
/// checkout with no such change and report that there is nothing to apply. Copied, not moved:
/// the root keeps its copy, so the panel still shows the change and its checklist, and the
/// worktree's copy is committed with the work.
///
/// And a **base** copy beside it, under `.cide/spec-carried/<checkout>/base/`: the folder
/// exactly as it was carried. Integrate compares the root's copy with it — a file the user has
/// not touched since is dropped when the branch's version lands, and one they edited while the
/// session worked is kept (see [`settle_parked`]). Without the base the two cannot be told apart,
/// and the root's copy is always "different" from the branch's, which has the ticks.
///
/// Not only Apply calls this: an ordinary dispatch whose task names a change carries it into the
/// run's `<role>-<task>` checkout at the fork (`agents::start_child`), for the same reason —
/// the run's own prompt points at `openspec/changes/<change>/`, and a worktree cut from `HEAD`
/// would not hold an uncommitted proposal. Hence the two skips:
///
/// * the root has no such folder (anymore — it was archived there, or the link dangles), and
/// * the **checkout** has already archived the change: re-forking a run whose branch archived it
///   would otherwise resurrect the folder beside its own archive entry.
pub(crate) fn carry_change(
    root: &Path,
    checkout: &Path,
    change: &str,
    carried: &Path,
) -> Result<()> {
    let from = root.join("openspec/changes").join(change);
    let to = checkout.join("openspec/changes").join(change);
    if !from.is_dir()
        || cide_spec::archived_dir(checkout, &cide_ipc::ChangeName(change.into())).is_some()
    {
        return Ok(());
    }
    let failed = |error: std::io::Error| {
        CoreError::Io(format!(
            "could not copy {change} into its worktree: {error}"
        ))
    };
    let base = carried.join("base").join("openspec/changes").join(change);
    if to.is_dir() {
        // Apply may have been launched while Propose was still writing. Carry documents added
        // since that first copy, preserving existing edits and files deliberately deleted from
        // the worktree. A checkout with no carried base belongs to its own author.
        if base.is_dir() {
            copy_new_artifacts(&from, &to, &base).map_err(failed)?;
        }
        return Ok(());
    }
    copy_dir(&from, &to).map_err(failed)?;
    let _ = std::fs::remove_dir_all(&base);
    copy_dir(&from, &base).map_err(failed)
}

fn copy_new_artifacts(from: &Path, to: &Path, base: &Path) -> std::io::Result<()> {
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        let original = base.join(entry.file_name());
        match std::fs::symlink_metadata(&target) {
            Ok(existing) => {
                if kind.is_dir() && existing.is_dir() {
                    copy_new_artifacts(&entry.path(), &target, &original)?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if original.exists() {
                    continue;
                }
                if kind.is_dir() {
                    copy_dir(&entry.path(), &target)?;
                    copy_dir(&entry.path(), &original)?;
                } else if kind.is_file() {
                    std::fs::create_dir_all(to)?;
                    std::fs::create_dir_all(base)?;
                    std::fs::copy(entry.path(), target)?;
                    std::fs::copy(entry.path(), original)?;
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// `.cide/spec-carried/<checkout>`: the base copy [`carry_change`] made, and what Integrate parks.
pub(crate) fn carried_dir(root: &Path, checkout: &str) -> PathBuf {
    root.join(cide_agents::config::CIDE_DIR)
        .join("spec-carried")
        .join(checkout)
}

/// `openspec/changes/<change>` relative to a root, when `change` is a name OpenSpec's grammar
/// allows — the folder [`carry_change`] copies and [`integrate_parked`] parks. Anything else is
/// `None`: a name with a separator in it must not be joined onto a root.
pub(crate) fn change_folder(change: &str) -> Option<PathBuf> {
    cide_spec::claude::valid_name(change).then(|| PathBuf::from("openspec/changes").join(change))
}

/// Integrate `name`'s branch with the root's untracked copy of `openspec/changes/<change>/`
/// parked out of the way first, and settle the park once the merge lands. `None` — or a name
/// that is not OpenSpec's kebab grammar — is a plain integrate.
///
/// The OpenSpec case on **every** integrate road, not just the spec panel's: a change proposed
/// in the root stays uncommitted there, and the run's branch carries the copy that
/// [`carry_change`] put into its checkout. Merging that branch back then meets the root's own
/// untracked copy of every one of those files, and git's safe checkout refuses — "N conflicts
/// prevent checkout" — over files the branch is the continuation of. Parked for the merge, put
/// back if it does not land, settled once it has (see [`settle_parked`]).
///
/// Answers the merge outcome and the **kept** paths — a root copy the user edited while the run
/// worked, left in the park and named by the caller.
pub(crate) fn integrate_parked(
    root: &Path,
    name: &str,
    change: Option<&str>,
) -> std::result::Result<(cide_git::worktree::Integration, Vec<String>), cide_ipc::git::GitError> {
    let Some(change_dir) = change.and_then(change_folder) else {
        return cide_git::worktree::integrate(root, name).map(|outcome| (outcome, Vec::new()));
    };
    let carried = carried_dir(root, name);
    let park = carried.join("parked");
    let parked = cide_git::worktree::park_untracked(root, name, &change_dir, &park)?;
    let merged = cide_git::worktree::integrate(root, name);
    let kept = match &merged {
        Ok(cide_git::worktree::Integration::Merged { .. })
        | Ok(cide_git::worktree::Integration::UpToDate) => settle_parked(root, &carried, &parked),
        _ => {
            cide_git::worktree::unpark(root, &park, &parked);
            Vec::new()
        }
    };
    merged.map(|merged| (merged, kept))
}

/// After a merge that landed: drop each parked file the user never touched (it is byte for byte
/// the base copy Apply carried), keep the rest where they are parked, and answer the kept ones as
/// paths under the root. The base goes either way — the change is in the branch now.
pub(crate) fn settle_parked(root: &Path, carried: &Path, parked: &[PathBuf]) -> Vec<String> {
    let park = carried.join("parked");
    let base = carried.join("base");
    let mut kept = Vec::new();
    for rel in parked {
        let at = park.join(rel);
        let untouched = match (std::fs::read(&at), std::fs::read(base.join(rel))) {
            (Ok(mine), Ok(carried)) => mine == carried,
            _ => false,
        };
        if untouched {
            let _ = std::fs::remove_file(&at);
        } else if let Ok(shown) = at.strip_prefix(root) {
            kept.push(shown.display().to_string());
        }
    }
    let _ = std::fs::remove_dir_all(&base);
    if kept.is_empty() {
        let _ = std::fs::remove_dir_all(carried);
    }
    kept
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Every OpenSpec session of a project, newest first.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_runs(
    agents: State<'_, Arc<AgentRegistry>>,
    project: ProjectId,
) -> Result<Vec<SpecRunRow>> {
    Ok(agents.spec_runs(project))
}

/// Dismiss an ended session from the panel: the registry forgets it, so it leaves the panel and
/// the Agents history. Refused, with the sentence, for a session that is still going.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_session_dismiss(
    app: AppHandle,
    agents: State<'_, Arc<AgentRegistry>>,
    project: ProjectId,
    run: RunId,
) -> Result<()> {
    let dismissed = agents
        .forget_spec_run(project, run)
        .map_err(CoreError::Io)?;
    agents.mark_changed(&app, project);
    // A session still alive — a Claude waiting at its prompt — is hidden already; stop its child
    // now, forcefully: the user has said they do not need it, and there is no turn to wind down.
    if dismissed == crate::agents::Dismissed::Stopping {
        crate::cmd::agents::agents_stop_with(
            app,
            project,
            run,
            crate::agents::StopBy::User,
            None,
            true,
        )
        .await?;
    }
    Ok(())
}

/// Every `spec-<change>` worktree the project has, read from git.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_checkouts(
    state: State<'_, WorkspaceState>,
    boards: State<'_, Arc<crate::spec_state::SpecBoards>>,
    project: ProjectId,
) -> Result<Vec<SpecCheckout>> {
    let root = tasks_state::project_root(&state, project)?;
    let reader = boards.project(project);
    blocking(move || {
        let mut rows = reader
            .checkout_metadata(|| checkout_metadata(&root))
            .map_err(CoreError::Io)?;
        reader.retain_checkouts(
            &rows
                .iter()
                .map(|row| cide_git::worktree::path_of(&root, &checkout_for(&row.change.0)))
                .collect::<Vec<_>>(),
        );
        for row in &mut rows {
            let cwd = cide_git::worktree::path_of(&root, &checkout_for(&row.change.0));
            if !cwd.join("openspec/changes").join(&row.change.0).is_dir() {
                row.archived_as = cide_spec::archived_dir(&cwd, &row.change).and_then(|path| {
                    path.file_name()
                        .map(|name| name.to_string_lossy().to_string())
                });
            }
            if row.archived_as.is_none()
                && let Ok((done, total)) = reader.progress(&cwd, &row.change)
            {
                row.completed_tasks = Some(done);
                row.total_tasks = Some(total);
            }
        }
        Ok(rows)
    })
    .await
}

fn checkout_metadata(root: &Path) -> Vec<SpecCheckout> {
    cide_git::worktree::list(root)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|tree| {
            let change = tree.agent.strip_prefix("spec-")?.to_string();
            Some(SpecCheckout {
                completed_tasks: None,
                total_tasks: None,
                archived_as: None,
                branch: format!("{}/{}", cide_git::worktree::BRANCH_PREFIX, tree.agent),
                unmerged: cide_git::worktree::unmerged(root, &tree.agent)
                    .ok()
                    .map(|n| n.unwrap_or(0) as u32),
                dirty: cide_git::worktree::dirty(root, &tree.agent).unwrap_or(0) > 0,
                change: ChangeName(change),
            })
        })
        .collect()
}

/// The sentence refusing a gesture on a change whose session is still working, or `None`.
///
/// Idle is allowed: a session that handed its turn back is the ordinary state when the user
/// decides the change is done.
fn still_working(agents: &AgentRegistry, project: ProjectId, change: &str) -> Option<String> {
    agents
        .spec_runs(project)
        .into_iter()
        .find(|row| {
            row.change.as_ref().is_some_and(|c| c.0 == change)
                && (row.state.counts_as_work() || matches!(row.state, RunState::Queued))
        })
        .map(|row| {
            format!(
                "{} is still working on {change}; wait for it to finish its turn, or stop it",
                row.label
            )
        })
}

/// Publish: commit what the session left uncommitted, and push `cide/spec-<change>` with its
/// upstream set, so it can be merged through the forge. Archive is not part of it.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_publish(
    state: State<'_, WorkspaceState>,
    agents: State<'_, Arc<AgentRegistry>>,
    project: ProjectId,
    change: ChangeName,
) -> Result<SpecPublished> {
    let root = tasks_state::project_root(&state, project)?;
    if let Some(reason) = still_working(&agents, project, &change.0) {
        return Ok(SpecPublished::Refused { reason });
    }
    let proxy = crate::cmd::git::git_proxy(&state);
    blocking(move || {
        let change = checked_change(&change.0)?.to_string();
        let name = checkout_for(&change);
        let checkout = cide_git::worktree::path_of(&root, &name);
        if !checkout.is_dir() {
            return Ok(SpecPublished::Refused {
                reason: format!(
                    "{change} was not applied in a worktree, so there is no branch to publish"
                ),
            });
        }
        let committed = cide_git::worktree::commit_all(&root, &name, &commit_message(&change))
            .map_err(|error| CoreError::Io(error.to_string()))?;
        let outcome = cide_git::push::push(
            &checkout,
            &cide_ipc::git::PushRequest {
                remote: None,
                refspec: None,
                set_upstream: true,
                force: false,
            },
            &proxy,
        );
        Ok(match outcome {
            Ok(outcome) => SpecPublished::Pushed {
                remote: outcome.remote,
                branch: if outcome.branch.is_empty() {
                    format!("{}/{name}", cide_git::worktree::BRANCH_PREFIX)
                } else {
                    outcome.branch
                },
                committed,
            },
            Err(error) => SpecPublished::Refused {
                reason: format!("the push failed: {error}"),
            },
        })
    })
    .await
}

fn commit_message(change: &str) -> String {
    format!("OpenSpec: {change}")
}

/// Integrate: commit what the session left uncommitted, then merge `cide/spec-<change>` into the
/// branch the project has checked out — with the project's verify and guards, as every
/// integrate. Archive is not part of it.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_integrate(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    agents: State<'_, Arc<AgentRegistry>>,
    boards: State<'_, Arc<crate::spec_state::SpecBoards>>,
    project: ProjectId,
    change: ChangeName,
) -> Result<SpecIntegrated> {
    let root = tasks_state::project_root(&state, project)?;
    if let Some(reason) = still_working(&agents, project, &change.0) {
        return Ok(SpecIntegrated::Refused { reason });
    }
    let change = checked_change(&change.0)?.to_string();
    let change_name = change.clone();
    let name = checkout_for(&change);

    let root_for_work = root.clone();
    let name_for_work = name.clone();
    let committed = blocking(move || {
        if !cide_git::worktree::path_of(&root_for_work, &name_for_work).is_dir() {
            return Ok(None);
        }
        cide_git::worktree::commit_all(&root_for_work, &name_for_work, &commit_message(&change))
            .map(Some)
            .map_err(|error| CoreError::Io(error.to_string()))
    })
    .await?;
    if committed.is_none() {
        return Ok(SpecIntegrated::Refused {
            reason: "this change was not applied in a worktree, so there is nothing to integrate"
                .into(),
        });
    }

    // The verify and the guarded paths are the project's rule for anything merged into its
    // branch, not a milestone's — so they run whether the tracker is on or not.
    let app_for_work = app.clone();
    let root_for_work = root.clone();
    let name_for_work = name.clone();
    let checked = blocking(move || {
        Ok(crate::milestones::before_integrate_at(
            &app_for_work,
            project,
            &root_for_work,
            &cide_ipc::AgentId("spec".into()),
            None,
            name_for_work,
        ))
    })
    .await?;
    if let Err(reason) = checked {
        return Ok(SpecIntegrated::Refused { reason });
    }

    let root_for_work = root.clone();
    let name_for_work = name.clone();
    let merged = blocking(move || {
        // The root's untracked copy of the change folder — the proposal, never committed, that
        // Apply carried into the worktree (`carry_change`) — would make git refuse the checkout
        // ("N conflicts prevent checkout") over files the branch is the continuation of. Parked
        // for the merge, put back if it does not land, dropped once it has.
        integrate_parked(&root_for_work, &name_for_work, Some(&change_name))
            .map_err(|error| CoreError::Io(error.to_string()))
    })
    .await?;
    let (merged, kept) = merged;
    let outcome = match merged {
        cide_git::worktree::Integration::UpToDate => SpecIntegrated::UpToDate,
        cide_git::worktree::Integration::Merged { commit, files } => SpecIntegrated::Merged {
            commit: Some(commit),
            files: files as u32,
            kept,
        },
        cide_git::worktree::Integration::Conflicts { paths } => {
            return Ok(SpecIntegrated::Conflicts { paths });
        }
    };
    // Landed: the worktree goes when nothing in it would be lost and nobody stands in it.
    crate::agents::retire_worktree(&app, &agents, project, name);
    boards.mark_changed(&app, project);
    Ok(outcome)
}

/// Where a change's Archive should run: its worktree while that still holds work the project's
/// branch lacks (the archive then travels with Publish or Integrate), the project root otherwise.
pub(crate) fn archive_dir(root: &Path, change: &str) -> PathBuf {
    let name = checkout_for(change);
    let checkout = cide_git::worktree::path_of(root, &name);
    let waiting = checkout.join("openspec/changes").join(change).is_dir()
        && (cide_git::worktree::unmerged(root, &name)
            .ok()
            .flatten()
            .is_some()
            || cide_git::worktree::dirty(root, &name).unwrap_or(0) > 0);
    if waiting {
        checkout
    } else {
        root.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_proposals_use_the_installed_workflow_and_keep_implementation_separate() {
        let root =
            std::env::temp_dir().join(format!("cide-task-proposal-{}", uuid::Uuid::new_v4()));
        let skill = root.join(".claude/skills/openspec-propose/SKILL.md");
        std::fs::create_dir_all(skill.parent().expect("parent")).expect("scratch");
        std::fs::create_dir_all(root.join("openspec")).expect("spec root");
        std::fs::write(
            &skill,
            "---\nname: openspec-propose\n---\nWrite a proposal.",
        )
        .expect("skill");
        let store = cide_tasks::TaskStore::open(&root);
        let project = ProjectId::new();
        let req: cide_ipc::TaskNew = serde_json::from_value(serde_json::json!({
            "project": project, "title": "Dark mode", "body": "Follow system\npreferences"
        }))
        .expect("request");
        let task = store
            .create(&req, cide_ipc::TaskAuthor::User)
            .expect("task");
        for harness in [Harness::Claude, Harness::Codex] {
            let planned = plan_task_proposal(&root, project, &task, harness).expect("proposal");
            assert!(
                planned.task.is_none(),
                "no implementation binding or status lifecycle"
            );
            assert!(
                planned.checkout.is_none(),
                "proposal is visible in the project root"
            );
            assert!(!planned.prompt.contains('\n'), "one submitted turn");
            assert!(planned.prompt.contains("Follow system preferences"));
            assert!(planned.prompt.contains("mcp__cide__cide_task_update"));
            let RunPurpose::Spec(purpose) = planned.purpose else {
                panic!("OpenSpec purpose")
            };
            assert_eq!(purpose.task.as_ref(), Some(&task.id));
            assert!(purpose.brief.contains("link the resulting change"));
            assert!(
                !purpose
                    .brief
                    .contains("Do not create tasks or look for a task board")
            );
            let reloaded: SpecPurpose =
                serde_json::from_str(&serde_json::to_string(&purpose).expect("persist"))
                    .expect("restore");
            assert_eq!(reloaded.task, purpose.task, "task tools survive restart");
        }
        assert!(
            store.get(&task.id).expect("task").change.is_none(),
            "planning did not scaffold a stub or invent a link"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_change_name_that_is_not_kebab_is_refused_before_it_names_a_path() {
        assert!(checked_change("add-dark-mode").is_ok());
        for bad in ["../x", "Add", "a b", "", &"x".repeat(60)] {
            assert!(checked_change(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_uncommitted_change_is_carried_into_a_fresh_worktree() {
        let dir = std::env::temp_dir().join(format!("cide-spec-carry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("root");
        let tree = dir.join("tree");
        std::fs::create_dir_all(root.join("openspec/changes/c1/specs/a")).unwrap();
        std::fs::write(root.join("openspec/changes/c1/tasks.md"), "- [ ] one\n").unwrap();
        std::fs::write(root.join("openspec/changes/c1/specs/a/spec.md"), "x").unwrap();
        std::fs::create_dir_all(&tree).unwrap();
        let carried = dir.join("carried");
        carry_change(&root, &tree, "c1", &carried).unwrap();
        assert!(tree.join("openspec/changes/c1/tasks.md").is_file());
        assert!(
            carried.join("base/openspec/changes/c1/tasks.md").is_file(),
            "and the base copy Integrate compares against"
        );
        assert!(tree.join("openspec/changes/c1/specs/a/spec.md").is_file());
        // A second carry leaves the worktree's own (possibly edited) copy alone.
        std::fs::write(tree.join("openspec/changes/c1/tasks.md"), "- [x] one\n").unwrap();
        carry_change(&root, &tree, "c1", &carried).unwrap();
        assert_eq!(
            std::fs::read_to_string(tree.join("openspec/changes/c1/tasks.md")).unwrap(),
            "- [x] one\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retrying_an_early_apply_carries_new_artifacts_and_preserves_worktree_edits() {
        let dir =
            std::env::temp_dir().join(format!("cide-spec-carry-late-{}", uuid::Uuid::new_v4()));
        let root = dir.join("root");
        let tree = dir.join("tree");
        let carried = dir.join("carried");
        let source = root.join("openspec/changes/c1");
        let target = tree.join("openspec/changes/c1");
        let base = carried.join("base/openspec/changes/c1");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("proposal.md"), "early proposal").unwrap();
        std::fs::write(source.join("deleted.md"), "known artifact").unwrap();
        carry_change(&root, &tree, "c1", &carried).unwrap();

        std::fs::write(target.join("proposal.md"), "worktree edit").unwrap();
        std::fs::remove_file(target.join("deleted.md")).unwrap();
        std::fs::write(source.join("proposal.md"), "completed proposal").unwrap();
        std::fs::write(source.join("tasks.md"), "- [ ] one\n").unwrap();
        std::fs::write(source.join("design.md"), "design").unwrap();
        std::fs::create_dir_all(source.join("specs/a")).unwrap();
        std::fs::write(source.join("specs/a/spec.md"), "requirement").unwrap();
        carry_change(&root, &tree, "c1", &carried).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("proposal.md")).unwrap(),
            "worktree edit"
        );
        assert_eq!(
            std::fs::read_to_string(base.join("proposal.md")).unwrap(),
            "early proposal"
        );
        assert!(
            !target.join("deleted.md").exists(),
            "a deliberately deleted artifact stays deleted"
        );
        for file in ["tasks.md", "design.md", "specs/a/spec.md"] {
            assert_eq!(
                std::fs::read(target.join(file)).unwrap(),
                std::fs::read(source.join(file)).unwrap()
            );
            assert_eq!(
                std::fs::read(base.join(file)).unwrap(),
                std::fs::read(source.join(file)).unwrap()
            );
        }
        std::fs::write(target.join("tasks.md"), "- [x] one\n").unwrap();
        std::fs::create_dir_all(source.join("specs/b")).unwrap();
        std::fs::write(source.join("specs/b/spec.md"), "another requirement").unwrap();
        carry_change(&root, &tree, "c1", &carried).unwrap();
        assert_eq!(
            std::fs::read_to_string(target.join("tasks.md")).unwrap(),
            "- [x] one\n"
        );
        assert!(
            target.join("specs/b/spec.md").is_file(),
            "new nested artifacts are carried too"
        );
        assert!(base.join("specs/b/spec.md").is_file());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The two skips the ordinary-dispatch carry needs. A run re-forked (resume, a checkout
    /// deleted while it queued) must not fail because the root archived the change in the
    /// meantime, and must not **resurrect** the folder beside an archive entry the run's own
    /// branch made.
    #[test]
    fn a_carry_skips_when_there_is_nothing_to_carry_or_something_to_resurrect() {
        let dir =
            std::env::temp_dir().join(format!("cide-spec-carry-skip-{}", uuid::Uuid::new_v4()));
        let root = dir.join("root");
        let tree = dir.join("tree");
        let carried = dir.join("carried");
        std::fs::create_dir_all(&tree).unwrap();
        // Nothing in the root: no folder, no base copy, no error.
        carry_change(&root, &tree, "c1", &carried).unwrap();
        assert!(!tree.join("openspec/changes/c1").exists());
        assert!(!carried.join("base").exists());
        // The checkout has archived the change: the root's copy stays out of it.
        std::fs::create_dir_all(root.join("openspec/changes/c1")).unwrap();
        std::fs::write(root.join("openspec/changes/c1/tasks.md"), "- [ ] one\n").unwrap();
        let archived = tree.join("openspec/changes/archive/2026-10-04-c1");
        std::fs::create_dir_all(&archived).unwrap();
        std::fs::write(archived.join("tasks.md"), "- [x] one\n").unwrap();
        carry_change(&root, &tree, "c1", &carried).unwrap();
        assert!(archived.is_dir());
        assert!(
            !tree.join("openspec/changes/c1").exists(),
            "the archive stands; the folder is not resurrected beside it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The reported bug's other half, at the helper's level: a proposal uncommitted in the
    /// root, carried into a role's checkout and committed there with its ticks. A plain
    /// integrate is refused — git's safe checkout will not write over the root's own untracked
    /// copy of every one of those files — while the parked one lands the branch's copy and
    /// drops the root's untouched one.
    #[test]
    fn integrating_a_carried_change_parks_the_root_copy_and_lands_the_branchs() {
        let root = std::env::temp_dir().join(format!("cide-spec-parked-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        git(&root, &["config", "user.name", "cide tests"]);
        git(&root, &["config", "user.email", "tests@cide.invalid"]);
        std::fs::write(root.join("base.txt"), "base\n").unwrap();
        git(&root, &["add", "-A"]);
        git(&root, &["commit", "-qm", "base"]);

        let wt = cide_git::worktree::ensure(&root, "developer-t1").expect("checkout");
        // The proposal, uncommitted in the root, carried into the checkout and ticked there.
        std::fs::create_dir_all(root.join("openspec/changes/c1")).unwrap();
        std::fs::write(root.join("openspec/changes/c1/tasks.md"), "- [ ] one\n").unwrap();
        carry_change(&root, &wt.path, "c1", &carried_dir(&root, "developer-t1")).unwrap();
        std::fs::write(wt.path.join("openspec/changes/c1/tasks.md"), "- [x] one\n").unwrap();
        git(&wt.path, &["add", "-A"]);
        git(&wt.path, &["commit", "-qm", "apply c1"]);

        // The bug, pinned: plain integrate refuses over the root's untracked copy.
        assert!(
            cide_git::worktree::integrate(&root, "developer-t1").is_err(),
            "git refuses the checkout; that refusal is the bug this helper exists to clear"
        );

        let (merged, kept) = integrate_parked(&root, "developer-t1", Some("c1")).expect("merge");
        assert!(
            matches!(merged, cide_git::worktree::Integration::Merged { .. }),
            "the branch lands once the copy is parked"
        );
        assert!(kept.is_empty(), "the root's copy was never touched");
        assert_eq!(
            std::fs::read_to_string(root.join("openspec/changes/c1/tasks.md")).unwrap(),
            "- [x] one\n",
            "the branch's copy, with its ticks, landed"
        );
        assert!(
            !carried_dir(&root, "developer-t1").exists(),
            "settled: the base and the park go with it"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Run git in a scratch repo with the user's config held out, so a global `user.name`, an
    /// alias or a hook on the test machine cannot change what these tests see.
    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "cide tests")
            .env("GIT_AUTHOR_EMAIL", "tests@cide.invalid")
            .env("GIT_COMMITTER_NAME", "cide tests")
            .env("GIT_COMMITTER_EMAIL", "tests@cide.invalid")
            .output()
            .unwrap_or_else(|error| panic!("running git {args:?}: {error}"));
        assert!(
            out.status.success(),
            "git {args:?} failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// A root copy the user never touched is dropped once the branch lands; one they edited while
    /// the session worked is kept, and named.
    #[test]
    fn an_edited_root_copy_is_kept_and_an_untouched_one_dropped() {
        let root = std::env::temp_dir().join(format!("cide-spec-settle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let carried = root.join(".cide/spec-carried/spec-c1");
        let base = carried.join("base/openspec/changes/c1");
        let park = carried.join("parked/openspec/changes/c1");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::create_dir_all(&park).unwrap();
        std::fs::write(base.join("tasks.md"), "- [ ] one\n").unwrap();
        std::fs::write(base.join("proposal.md"), "why\n").unwrap();
        std::fs::write(park.join("tasks.md"), "- [ ] one\n").unwrap();
        std::fs::write(park.join("proposal.md"), "why, edited by me\n").unwrap();
        let parked = vec![
            PathBuf::from("openspec/changes/c1/tasks.md"),
            PathBuf::from("openspec/changes/c1/proposal.md"),
        ];
        let kept = settle_parked(&root, &carried, &parked);
        assert_eq!(
            kept,
            vec![".cide/spec-carried/spec-c1/parked/openspec/changes/c1/proposal.md".to_string()]
        );
        assert!(!park.join("tasks.md").exists(), "untouched: dropped");
        assert!(park.join("proposal.md").exists(), "edited: kept");
        assert!(!carried.join("base").exists(), "the base goes either way");
        let _ = std::fs::remove_dir_all(&root);
    }
}
