//! Subagents: what roles a project has, whether it may run them, and running them. (M18)
//!
//! Fourteen commands, in four groups. Three answer questions about files in the project —
//! [`agents_roster`] for the picture, [`agents_config_get`] for the switch, [`agents_config_set`]
//! for flipping it — and six act on the run registry: [`agents_dispatch`], [`agents_stop`], and
//! the four of pause and resume ([`agents_pause`], [`agents_resume`], [`agents_retry_turn`],
//! [`agents_ack_stale_turn`]). Everything the first three answer with is derived from
//! `.cide/config.json` and `.cide/agents/*.md`, read fresh on every call; everything those six
//! touch lives in [`crate::agents::AgentRegistry`], which owns the queue, the slots and the
//! runs.
//!
//! Three more **write** a role definition — [`agents_draft`] to populate a form from one file,
//! [`agents_save`] to write it back, [`agents_delete`] to remove it — and they are the answer to
//! the only way there used to be of defining a role, which was to open an editor and get the
//! front-matter grammar right. `cide_agents::defs`' writer is the whole of the mechanism; these
//! three are a project root, a thread and an event. See the section header above
//! [`agents_draft`].
//!
//! The fourteenth, [`agents_integrate`], is the odd one out and belongs to no group: it reaches
//! `cide_git::worktree` and merges a role's branch into the branch the user has checked out. It
//! is here rather than in `cmd::git` because the thing it names is a *role*, and the panel that
//! offers it is this one.
//!
//! Five shapes here are decisions rather than habits.
//!
//! **Nothing is cached and nothing is mirrored into `Workspace`.** `cide_agents`' module header
//! makes the argument in full: those files are committed, so a teammate's commit or a
//! `git checkout` can change them under the running app, and a cached roster is a roster that is
//! wrong for as long as nobody happens to invalidate it. A read costs a `read_dir` and a handful
//! of small files. `cide_fs::filter` already watches `<root>/.cide`, and `crate::dotcide` turns
//! a change there into `AgentRegistry::mark_changed`, whose coalesced flush re-reads and
//! broadcasts — so a role file written by an agent or moved by a `git pull` reaches the panel
//! without the panel ever polling.
//!
//! **`unavailable` now means only a real fault.** It used to carry a synthetic
//! "dispatch arrives in a later slice" sentence on every role, because a role that reported
//! itself dispatchable would have drawn a Dispatch button that did nothing — the
//! listed-and-silently-inert state `cide_core::commands` has paid for twenty-four times. There is
//! a registry now, so the synthetic is gone and [`cide_agents::dispatch_refusal`] is the whole
//! answer. See [`wire_def`].
//!
//! **[`agents_dispatch`] enqueues and returns; it never waits on the run.** This is the module's
//! version of the `openDiff` rule: that tool blocking an agent's turn is a documented invariant
//! precisely because it is dangerous, and a dispatch that waited would let one wedged subagent
//! freeze whoever called it — including the orchestrating session, mid-turn, over the MCP socket.
//!
//! **[`agents_config_set`] is the only writer of `.cide/config.json` in this process.** It is
//! also the only place the git-repository gate is enforced, because it is the only place a
//! project can be turned on from — see [`worktree_refusal`].
//!
//! **Nothing here runs on the main thread.** Tauri 2 polls a synchronous command on the GTK loop,
//! and every one of these opens a directory, reads several files and (through
//! `cide_agents::defs::installed`) walks `PATH`. Every handler is `async` and hands its disk work
//! to [`blocking`], for `cmd::git`'s and `cmd::tasks`' reasons — see there.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cide_agents::config::{self, CideConfig};
use cide_agents::defs;
use cide_agents::harness::Harness;
use cide_agents::tools::Stopped;
use cide_agents::{Isolation, LoadedAgent, ProjectAgents};
use cide_core::{CoreError, workspace};
use cide_ipc::agents::{AgentDraft, AgentSaveOutcome, AgentScope};
use cide_ipc::{
    AgentDef, AgentId, AgentRoster, DispatchRequest, OrchestrationConfig, OrchestrationPatch,
    ProjectId, RunId, Task, TaskId,
};
use cide_tasks::TaskStore;
use tauri::{AppHandle, Manager, State};

use crate::agents::{AgentRegistry, DispatchSpec, HeldPair, StopBy, StopRequest};
use crate::tasks_state::TasksStores;
use crate::workspace_state::WorkspaceState;

type Result<T> = std::result::Result<T, CoreError>;

/// Run disk work on the blocking pool.
///
/// `#[tauri::command(async)]` is not the fix and neither is an `async fn` whose body never
/// awaits: both only move the call onto the async runtime, where a blocking `read_dir` still
/// occupies a runtime worker for its whole duration. `spawn_blocking` is the pool built for it,
/// and it is the same helper `cmd::git` and `cmd::tasks` each carry for the same reason.
///
/// The workspace lock is *not* taken in here. Callers resolve their root first, on the caller's
/// thread, and move a plain `PathBuf` in; a `State` guard held across the await would be both
/// un-`Send` and a lock held for the length of a file write.
async fn blocking<T>(work: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T>
where
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        // A panic in the worker is a bug, not a project condition — but it has to reach the panel
        // as *something*, or the command hangs on a channel that never answers.
        .map_err(|error| CoreError::Io(format!("the agents worker did not finish: {error}")))?
}

/// The project's first root, which is the directory `.cide/` lives in.
///
/// A private copy rather than a shared helper, matching `cmd::settings`' own: the four callers of
/// this idea across `cmd/` each want a different error type, and a common one would have to
/// invent a lowest common denominator that none of them wants.
fn project_root(state: &WorkspaceState, project: ProjectId) -> Result<PathBuf> {
    state.with(|ws| {
        workspace::project(ws, project)?
            .roots
            .first()
            .map(|root| root.path.clone())
            // A project with no roots fails `workspace::validate`, so this is belt-and-braces —
            // but the alternative to answering here is `roots[0]` on an empty vec, and a panic in
            // a command worker takes the process down under `panic = "abort"`.
            .ok_or(CoreError::NoRoots)
    })
}

/// What cide knows about this project's subagents right now.
///
/// Three shapes, not an array, on `AgentRoster`'s own argument: an empty list cannot say whether
/// orchestration is off, on with no roles defined, or on with roles and nothing running, and
/// those are three different screens.
#[tauri::command(rename_all = "camelCase")]
pub async fn agents_roster(
    state: State<'_, WorkspaceState>,
    agents: State<'_, Arc<AgentRegistry>>,
    project: ProjectId,
) -> Result<AgentRoster> {
    let root = project_root(&state, project)?;
    let runs = agents.runs_for(project);
    let dispatching = agents.dispatching(project);
    blocking(move || {
        Ok(roster(
            &root,
            runs,
            dispatching,
            crate::agents::cide_hook_binary().as_deref(),
        ))
    })
    .await
}

/// The three switches from `.cide/config.json` that the panel draws.
///
/// The two fields the file carries and this does not — `isolation` and
/// `allowDangerousPermissions` — are hand-edited by design; see `AgentsConfig::apply` for why a
/// round trip through the panel must not be able to reset them.
#[tauri::command(rename_all = "camelCase")]
pub async fn agents_config_get(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
) -> Result<OrchestrationConfig> {
    let root = project_root(&state, project)?;
    blocking(move || Ok(config::load(&root).agents.to_wire())).await
}

/// Change `.cide/config.json`, and tell every window what the project looks like now.
///
/// # Why it answers with the config rather than with `{ ok: true }`
///
/// `cmd::tasks`' argument, one panel over, and it is stronger here: there is no
/// `cide://workspace-changed` carrying a per-project file, so a caller that got an acknowledgement
/// would have exactly one way to learn what it had done — ask again — and the frame in between
/// shows the old switch position under a toggle the user has already moved.
///
/// # Why it emits as well
///
/// The event is for the *other* windows, and for nothing else. Two windows can be open on one
/// project, and a switch that flipped in one of them and not the other is a project that is
/// enabled and disabled at the same time depending on which window you look at.
#[tauri::command(rename_all = "camelCase")]
pub async fn agents_config_set(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    agents: State<'_, Arc<AgentRegistry>>,
    project: ProjectId,
    patch: OrchestrationPatch,
) -> Result<OrchestrationConfig> {
    let root = project_root(&state, project)?;
    let runs = agents.runs_for(project);
    let dispatching = agents.dispatching(project);

    let (config, roster) = blocking(move || {
        // Read-modify-write rather than serialise-from-scratch, because `config::write` merges
        // into whatever is already in the file: the two disk-only keys, and anything a future
        // cide added, survive a round trip through a panel that has never heard of them.
        let mut file: CideConfig = config::load(&root);
        file.agents.apply(patch);

        // Only the *gesture that enables* is gated. A project already switched on by hand keeps
        // working, and a patch that only changes `maxConcurrent` on it is not the moment to
        // refuse: refusing there would wedge the panel shut over a state the user wrote
        // themselves, while fixing nothing.
        if patch.enabled == Some(true)
            && let Some(why) = worktree_refusal(&root, &file)
        {
            // `CoreError::Io` because there is no tagged variant for a policy refusal and adding
            // one is a `cide-core` change outside this slice. Nothing branches on the tag here —
            // the sentence *is* the answer, and it names the two ways out.
            return Err(CoreError::Io(why));
        }

        config::write(&root, &file)?;
        // Built after the write, from the file that is now on disk, so the roster the other
        // windows are handed and the config this call answers with cannot disagree about what
        // just happened.
        Ok((
            file.agents.to_wire(),
            roster(
                &root,
                runs,
                dispatching,
                crate::agents::cide_hook_binary().as_deref(),
            ),
        ))
    })
    .await?;

    crate::emit::agents_changed(&app, project, &roster);
    Ok(config)
}

// ==========================================================================================
// Editing them, which until now meant editing markdown by hand.
// ==========================================================================================
//
// Three commands, and they are the first things in this module that **write** a definition.
// Everything above reads `.cide/agents/*.md` and answers questions about it; the only way to
// change one was an editor and a correct guess at the front-matter grammar.
//
// The rules they follow are the same three the rest of the module follows, and one more.
//
// * **Nothing is cached.** A save re-reads the directory afterwards, because the file it just
//   wrote is not the only one in there and a teammate's `git pull` may have moved another.
// * **The whole roster comes back**, as `agents_config_set` and `cmd::tasks` do, for the reason
//   `AgentSaveOutcome`'s doc gives: there is no `cide://workspace-changed` carrying a per-project
//   file, so an acknowledgement leaves the caller one round trip behind its own gesture.
// * **Every one emits**, so a second window on the same project does not keep drawing a role
//   that no longer exists. `.cide/` is watched and a project-scope write would arrive as an
//   `cide://fs-changed` eventually — but `$XDG_CONFIG_HOME/cide/agents/` is not watched by
//   anything, so for a global role this event is the only notice there is.
// * **A refusal is not an error.** `agents_save` answers `Rejected` with a problem per field, in
//   the `Ok` channel, exactly as `agents_integrate` answers `Conflicts`. `CoreError` has one
//   string per variant and a form needs eleven.

/// One definition file, as the form that edits it is populated.
///
/// # Why a command of its own, rather than fields on `AgentDef` or drafts on the roster
///
/// `AgentDef` is the roster row and deliberately carries neither `tools` nor `permission-mode`
/// nor `effort` — its own doc says a row cannot act on them and a panel handed them would have to
/// be trusted not to draw them as editable. Growing it to carry a whole editable definition
/// would put every project's every system prompt on `cide://agents-changed`, which is not a
/// settings stream: the run registry fires it as turns start and tool calls land, several times a
/// second across every window, and `emit::agents_changed`'s doc records that it deliberately
/// carries no revision *because* it is that chatty. Roles are edited by a person, one at a time,
/// after a deliberate click.
///
/// Freshness is the sharper half of the argument. A draft carried on a roster snapshot is as old
/// as the snapshot, and this file has three other writers — the user's editor, a `git checkout`,
/// an agent working in this repository. A form populated from a snapshot and saved would revert
/// whatever landed in between, silently, over a committed file. One read at the moment the form
/// opens is the only version of this that is honest.
///
/// `scope` is required rather than searched for, on `AgentScope`'s argument: one roster row can
/// be backed by two files, and a form that guessed would edit the one the user is not looking at.
#[tauri::command(rename_all = "camelCase")]
pub async fn agents_draft(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    agent: AgentId,
    scope: AgentScope,
) -> Result<AgentDraft> {
    // Resolved on the caller's thread, so no workspace guard crosses the await. See `blocking`.
    let root = project_root(&state, project)?;
    blocking(move || {
        defs::read_draft(&root, scope, agent.as_str())
            .map_err(|error| CoreError::Io(error.to_string()))
    })
    .await
}

/// What the role form may offer in its **Model** box, for one harness. (M43)
///
/// # Why this is a command and not a constant
///
/// Because for opencode the answer is a property of the user's machine: `opencode models` lists
/// whatever providers they have configured and authenticated, and a compiled-in menu would be
/// wrong in both directions — offering models they cannot reach, and omitting every local one
/// they can. `AgentModels` carries the argument in full.
///
/// Claude Code has no such command, so its harness answers with the aliases and says so. Both
/// roads end in the same shape, which is the point of asking the harness rather than matching on
/// the enum here — `cide_agents::harness::Harness::models` is defaulted to an empty list, so a
/// harness nobody has taught to enumerate degrades to exactly the free-text box that exists
/// today rather than to a compile error in this file.
///
/// # `problem` rather than `Err`
///
/// A missing `opencode` is not a failure of this call; it is a fact about the machine, and the
/// form must still draw a Model box the user can type into. So the sentence rides in the
/// payload and the `Err` channel is left for what it is for — a worker that did not come back.
/// `AgentModels::problem` states the same rule from the other side.
///
/// # It forks, so it goes to the pool
///
/// `blocking`, for that helper's stated reason, and the root is resolved on the caller's thread
/// so no workspace guard crosses the await. A project with no roots probes with no cwd rather
/// than refusing: the models a harness can offer are mostly a property of the machine, and
/// answering "this project has no roots" to somebody choosing a model would be an error about
/// something they did not ask.
/// This project's local agent overrides — harness, pool, model, per role. (M45)
///
/// Global-ish state read from the profile's config directory rather than the project, so it
/// answers for a project that is open without touching the checkout. Never fails: an absent or
/// unreadable file is an empty set, which `cide_core::persist::load_agent_overrides` states at
/// length and which is the right answer rather than a degraded one.
#[tauri::command(rename_all = "camelCase")]
pub async fn agent_overrides_get(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
) -> Result<cide_ipc::ProjectOverrides> {
    let root = project_root(&state, project)?;
    blocking(move || {
        let all =
            cide_core::persist::load_agent_overrides(&cide_core::persist::agent_overrides_path());
        Ok(all.project(&root.to_string_lossy()))
    })
    .await
}

/// Replace this project's overrides, and answer what was stored.
///
/// **Whole-project, not per row.** The screen holds one small table and sends it back entire, for
/// `SettingsPatch`'s reason one file over: a per-row command would need a delete verb, an
/// ordering, and a story about two windows editing the same table — where a whole-table write has
/// none of those and the file is a few hundred bytes.
///
/// An override that says nothing is **kept**, not dropped. `LlmSettings::cleaned`'s lesson: the
/// webview redraws from what Rust stored, so a row the backend discards is a row the user can
/// never open and fill in. `overrides::resolve` skips an empty one at the point of use.
#[tauri::command(rename_all = "camelCase")]
pub async fn agent_overrides_set(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    overrides: cide_ipc::ProjectOverrides,
) -> Result<cide_ipc::ProjectOverrides> {
    let root = project_root(&state, project)?;
    blocking(move || {
        let path = cide_core::persist::agent_overrides_path();
        let mut all = cide_core::persist::load_agent_overrides(&path);
        let key = root.to_string_lossy().to_string();
        // A project whose whole table is empty is removed rather than stored as an empty object,
        // so the file does not accumulate a row per project ever opened. That is safe where
        // dropping a *row* is not: nothing is being edited here, the user has cleared it.
        match overrides.all.is_empty() && overrides.roles.is_empty() {
            true => {
                all.projects.remove(&key);
            }
            false => {
                all.projects.insert(key.clone(), overrides.clone());
            }
        }
        cide_core::persist::save_agent_overrides(&path, &all).map_err(|error| {
            CoreError::Io(format!("the overrides could not be written: {error}"))
        })?;
        Ok(all.project(&key))
    })
    .await
}

/// Does this provider/model actually answer? One very small real turn. (M45)
///
/// **It spends quota**, which is why it is a button the user presses rather than anything
/// automatic, and why the screen says so beside it. Listing a model id proves only that opencode
/// *resolved* it: a wrong key, an endpoint that accepts connections and refuses completions, a
/// retired model and an expired plugin OAuth all list perfectly and fail on the first token.
///
/// Answers a verdict rather than rejecting, for [`cide_ipc::LlmModelTest`]'s stated reason, and
/// forks on the pool for `agents_models`' below.
#[tauri::command(rename_all = "camelCase")]
pub async fn llm_test_model(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    model: String,
) -> Result<cide_ipc::LlmModelTest> {
    let root = project_root(&state, project).ok();
    // Read on the caller's thread, like `agents_models` beside it, so no workspace guard crosses
    // the await. The document handed to the test is the one a run gets.
    let llm = state.with(|ws| Ok::<_, CoreError>(ws.settings.llm.clone()))?;
    blocking(move || {
        let (ok, detail) =
            match cide_agents::harness::opencode::test_model(root.as_deref(), &llm, &model) {
                Ok(detail) => (true, detail),
                Err(detail) => (false, detail),
            };
        Ok(cide_ipc::LlmModelTest { model, ok, detail })
    })
    .await
}

#[tauri::command(rename_all = "camelCase")]
pub async fn agents_models(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    harness: cide_ipc::Harness,
) -> Result<cide_ipc::agents::AgentModels> {
    let root = project_root(&state, project).ok();
    // Read on the caller's thread, like `root`, so no workspace guard crosses the await —
    // `crate::agents::facts` takes the same lock for the same reason. Without it the probe would
    // list only the providers the user configured by hand, and cide's own would be invisible in
    // the very dialog where a model is chosen. (M45)
    let llm = state.with(|ws| Ok::<_, CoreError>(ws.settings.llm.clone()))?;
    blocking(move || {
        // The registry is the same lookup the dispatch makes, so this cannot answer for a
        // harness the fork would refuse — `defs::implemented` makes the identical argument
        // against matching on the enum in a second place.
        let Some(implementation) = cide_agents::harness::for_kind(harness) else {
            return Ok(cide_ipc::agents::AgentModels {
                harness,
                models: Vec::new(),
                // `defs::implemented` answers the same question the `for_kind` miss just
                // answered, and it owns the sentence. Asking it rather than writing a second one
                // is what keeps a greyed role in the panel and this dialog saying one thing.
                problem: defs::implemented(harness),
            });
        };
        Ok(match implementation.models(root.as_deref(), &llm) {
            Ok(models) => cide_ipc::agents::AgentModels {
                harness,
                models,
                problem: None,
            },
            Err(problem) => cide_ipc::agents::AgentModels {
                harness,
                models: Vec::new(),
                problem: Some(problem),
            },
        })
    })
    .await
}

/// Write one role's definition file, and tell every window what the project looks like now.
///
/// Creates, edits, renames and moves between scopes — all four are one call, because all four are
/// "here is what this role should be, and here is the file it is today". `cide_agents::defs::save`
/// makes the distinction from `AgentDraft::original` and documents the order a rename happens in.
///
/// # Why a rejection is an `Ok`
///
/// See `AgentSaveOutcome`. In one line: a form needs to know *which box*, `CoreError` has one
/// string, and a refusal with a next action in it is an outcome rather than a failure — which is
/// the shape [`agents_integrate`] already uses for a merge conflict, one command over.
///
/// The `Err` channel still carries what it is for: no project roots, a disk that would not take
/// the write, a rename whose old file could not be removed. None of those is the user's form to
/// fix, and all of them read as sentences.
///
/// # It does not check whether subagents are enabled
///
/// Deliberately. Writing a role definition is the thing a person does *before* turning
/// orchestration on — the `Disabled` screen names the config file and the enable button, and a
/// project with the switch off and three roles ready is an ordinary, useful state. Gating the
/// editor on the switch would mean the only way to prepare a project is the hand-edited markdown
/// this command exists to replace.
#[tauri::command(rename_all = "camelCase")]
pub async fn agents_save(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    agents: State<'_, Arc<AgentRegistry>>,
    project: ProjectId,
    draft: AgentDraft,
) -> Result<AgentSaveOutcome> {
    let root = project_root(&state, project)?;
    let runs = agents.runs_for(project);
    let dispatching = agents.dispatching(project);

    let outcome = blocking(move || match defs::save(&root, &draft) {
        // Built after the write, from the directory as it now stands, so the roster this answers
        // with and the roster the other windows are handed cannot disagree about what happened.
        Ok(path) => Ok(AgentSaveOutcome::Saved {
            path,
            roster: roster(
                &root,
                runs,
                dispatching,
                crate::agents::cide_hook_binary().as_deref(),
            ),
        }),
        Err(defs::WriteError::Rejected(problems)) => Ok(AgentSaveOutcome::Rejected { problems }),
        // Including `Stranded`, which is a *succeeded* save whose rename could not finish. It is
        // an `Err` because the user has something to do about it, and its sentence says in so
        // many words that the definition was written — otherwise the natural reading of a failed
        // save is "press it again", which would then refuse for a name that is now taken by the
        // file this call wrote. The panel refreshes from the `.cide/` watcher either way.
        Err(error) => Err(CoreError::Io(error.to_string())),
    })
    .await?;

    if let AgentSaveOutcome::Saved { roster, .. } = &outcome {
        crate::emit::agents_changed(&app, project, roster);
    }
    Ok(outcome)
}

/// Remove one role's definition file.
///
/// Scoped for `AgentScope`'s reason, and it matters more here than anywhere else in the module: a
/// project definition shadowing a global one draws **one row**, and a delete that guessed would
/// take the global file — which is not in any repository's history, is shared by every project
/// the user opens, and has nothing to be restored from.
///
/// Answers with the whole roster rather than `()`, for the reason the module header gives: the
/// caller's next frame has to draw a list that no longer contains this role, and asking again is
/// a round trip it would spend showing the row it just deleted.
///
/// There is no confirmation gate here, deliberately. A guard in the domain is what
/// `CoreError::UnsavedChanges` is for and it exists because a dropped promise must not be able to
/// lose a buffer — but this deletes a file in a git repository, which is the one place a mistake
/// is already recoverable, and a modal in Rust would also have to be a modal in the MCP tool
/// vocabulary and in every other door onto the same act. The confirmation belongs to the button.
#[tauri::command(rename_all = "camelCase")]
pub async fn agents_delete(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    agents: State<'_, Arc<AgentRegistry>>,
    project: ProjectId,
    agent: AgentId,
    scope: AgentScope,
) -> Result<AgentRoster> {
    let root = project_root(&state, project)?;
    let runs = agents.runs_for(project);
    let dispatching = agents.dispatching(project);

    let roster = blocking(move || {
        defs::delete(&root, agent.as_str(), scope)
            .map_err(|error| CoreError::Io(error.to_string()))?;
        Ok(roster(
            &root,
            runs,
            dispatching,
            crate::agents::cide_hook_binary().as_deref(),
        ))
    })
    .await?;

    crate::emit::agents_changed(&app, project, &roster);
    Ok(roster)
}

// ==========================================================================================
// Running them.
// ==========================================================================================

/// Dispatch one run, and answer as soon as it is on the queue.
///
/// # It never waits on the run, and that is a rule rather than an optimisation
///
/// `cide-app/src/ide.rs`'s `openDiff` is this application's standing example of a call that
/// blocks an agent's turn, and every early return in its pump has to cancel the request first
/// precisely because the failure mode — a `claude` waiting for ever with nothing on screen — is
/// so bad. A dispatch is reachable from the same place (`cide_agent_dispatch` over the agent-RPC
/// socket, inside the orchestrator's own turn) and would have exactly that shape if it waited:
/// one subagent wedged in a permission prompt would freeze the session that dispatched it.
///
/// So this composes the prompt, refuses what has to be refused, puts the run on its role's queue
/// and returns the [`RunId`]. Everything after that — the worktree, the argv, the fork — happens
/// on [`AgentRegistry::pump`]'s tasks, and the panel learns about it through
/// `cide://agents-changed`.
///
/// # Where the refusal happens
///
/// Here, in [`plan_dispatch`], **before a worktree is created** — and again at the spawn, because
/// a queued run can wait minutes and a `git pull` can disable the project in between. Refusing
/// early is what keeps a disabled project from acquiring `.cide/worktrees/` as a side effect of a
/// button that was going to fail anyway.
#[tauri::command(rename_all = "camelCase")]
pub async fn agents_dispatch(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    agents: State<'_, Arc<AgentRegistry>>,
    tasks: State<'_, Arc<TasksStores>>,
    request: DispatchRequest,
) -> Result<RunId> {
    match dispatch_or_duplicate(app, state, agents, tasks, request).await? {
        DispatchOutcome::Started(run) => Ok(run),
        // Flattened to the sentence, because both callers of *this* function asked for the
        // dispatch out loud: the panel's button and `cide_agent_dispatch`. The one caller that
        // did not ask — auto-dispatch, where a repeated assignment is an ordinary gesture rather
        // than a request — calls `dispatch_or_duplicate` directly and reads the tag.
        DispatchOutcome::Duplicate { why, .. } => Err(CoreError::Io(why)),
    }
}

/// What a dispatch did, for the one caller that must tell a duplicate from a failure. (M66)
///
/// [`agents_dispatch`] flattens this to `Err(CoreError::Io(why))`. [`crate::task_triggers`] does
/// not: an assignment repeated while a run lives is not a failure of anything — it is that
/// module's "everything declines by doing nothing" policy meeting a guard that finally exists —
/// and a `warn!` there would put a frightening line in the log for the ordinary case of a user
/// re-picking the assignee already on the task.
///
/// A tag rather than a [`CoreError`] variant, deliberately. `CoreError` is a `ts-rs` type whose
/// struct variants reach the webview as `{ kind, detail: { … } }`, and `ui/src/ipc/errorText.ts`
/// prints `detail` only when it is a **string** — so a `DuplicateRun { run, state }` would have
/// shown the panel the literal word `duplicateRun`, which is the `[object Object]` failure that
/// file's header was written about. The sentence rides `Io` like every other refusal
/// [`plan_dispatch`] composes, and the discriminator stays inside Rust where only Rust needs it.
pub(crate) enum DispatchOutcome {
    Started(RunId),
    Duplicate { held: HeldPair, why: String },
}

/// The whole of [`agents_dispatch`] except the flattening. See [`DispatchOutcome`].
pub(crate) async fn dispatch_or_duplicate(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    agents: State<'_, Arc<AgentRegistry>>,
    tasks: State<'_, Arc<TasksStores>>,
    request: DispatchRequest,
) -> Result<DispatchOutcome> {
    let project = request.project;
    let root = project_root(&state, project)?;
    // `ensure` rather than `get`, for `cmd::tasks::tracker`'s reason: the restore loop warms a
    // store per project but that list is capped, and a project past the cap would otherwise be
    // undispatchable with nothing anywhere saying why.
    let store = tasks.ensure(project, &root);
    // Cloned out of the `State` guard before the await: a guard held across one is un-`Send`, and
    // the registry is behind an `Arc` for exactly this — see `lib.rs`.
    let registry = Arc::clone(&agents);
    let lookup = Arc::clone(&agents);

    let spec = blocking(move || {
        // Read on the worker rather than here, for this module's standing reason: the registry's
        // lock is also taken by the hook applier thread, and this command is the one an
        // orchestrating session holds its turn behind.
        let held = request
            .task
            .as_ref()
            .and_then(|task| lookup.run_holding(project, &request.agent, task));
        plan_dispatch(&root, &store, held.as_ref(), &request)
    })
    .await?;

    // Named before the move: on the duplicate arm the spec is gone and the sentence wants them.
    let (agent, task) = (spec.agent.clone(), spec.task.clone());
    let run = match registry.enqueue_unique(spec) {
        Ok(run) => run,
        // The window the answer above could not cover — see `AgentRegistry::enqueue_unique`.
        // Composed through the same function, so a user cannot tell which gate fired.
        Err(held) => {
            let why = duplicate_refusal(&agent, task.as_ref(), &held);
            return Ok(DispatchOutcome::Duplicate { held, why });
        }
    };
    registry.mark_changed(&app, project);
    // Returns at once; each admitted run starts on its own task.
    registry.pump(&app);
    Ok(DispatchOutcome::Started(run))
}

/// Stop a run, from the **Agents panel**: cancel it if it is queued, kill its child if it has
/// one, now.
///
/// # This road does not ask, and `cide_agent_stop` does — deliberately
///
/// The panel's button is the outright kill it has always been. The tool asks the run to wind
/// down first, gives it the project's grace to write what it knows onto its task, and kills it
/// only if that runs out (M67). Two routes to one word meaning two things is normally the split
/// `openPushDialog` exists to prevent, and it is a deliberate choice here rather than an
/// oversight: a person at the panel pressing Stop on a run they are watching wants it to stop,
/// and has the row in front of them to see that it did. If this ever grows a wind-down, the
/// affordance for forcing is a second press on an already-stopping row — `stop_route` already
/// answers `Kill` for that — and **not** a modifier, which would be an undiscoverable gesture
/// for a destructive act.
///
/// `async` although it touches no disk, for the reason at the top of this module: a synchronous
/// command is polled on the GTK loop, and this takes a lock the hook thread also takes.
#[tauri::command(rename_all = "camelCase")]
pub async fn agents_stop(app: AppHandle, project: ProjectId, run: RunId) -> Result<()> {
    agents_stop_with(app, project, run, StopBy::User, None, true).await?;
    Ok(())
}

/// The whole of a stop, for both doors. See [`agents_stop`] and `AgentSink::stop`.
///
/// **The grace is read here**, on a worker, and handed to the registry as a value — that module
/// takes a lock the hook applier also takes and nothing under it may touch a disk. Read fresh at
/// each stop rather than cached at dispatch, on `nudge_orchestrator`'s rule: `.cide/config.json`
/// is committed, so a teammate's commit or a `git checkout` can change the number under a
/// running app, and a deadline computed at dispatch would honour one nobody now has.
pub(crate) async fn agents_stop_with(
    app: AppHandle,
    project: ProjectId,
    run: RunId,
    by: StopBy,
    reason: Option<String>,
    force: bool,
) -> Result<Stopped> {
    let agents = app
        .try_state::<Arc<AgentRegistry>>()
        .map(|state| Arc::clone(&state))
        .ok_or_else(|| CoreError::Io("this window has no agent registry".into()))?;

    // A force never reads the file: the grace it would have found changes nothing, and a stop
    // is the gesture people reach for when something has gone wrong — it must not be able to
    // wait on a disk.
    let grace = if force {
        Duration::ZERO
    } else {
        let root = project_root(&app.state::<WorkspaceState>(), project)?;
        blocking(move || Ok(cide_agents::load_project(&root).config.agents.stop_grace()))
            .await
            // A project whose config cannot be read still stops; it stops the old way. A stop
            // that refused because a file was unparseable would be the worst possible moment
            // for this feature to be the thing in the way.
            .unwrap_or(Duration::ZERO)
    };

    let request = StopRequest {
        by,
        // One line, at the door: this text is typed into a TUI and the harness ends it with
        // `\r`, so an embedded newline would be another Enter — `opening_prompt`'s rule, and
        // the one every prompt path in this codebase inherits.
        reason: reason
            .map(|reason| one_line(&reason))
            .filter(|r| !r.is_empty()),
        force,
        grace,
    };
    agents.stop(&app, project, run, &request)
}

/// Close this project's dispatch queue and freeze its children — or freeze one run.
///
/// # One command with a nullable argument, not two
///
/// `run: None` is the **project scope**: the queue is shut *and* every live run is frozen, along
/// with the project's primary console session. `run: Some(..)` freezes that one run. They are one
/// command because they share a queue, a signal and a refusal path, and splitting them would put
/// two implementations of one refusal in the file — the same argument `agents_stop` makes for
/// covering a queued run and a live one.
///
/// The ordering the freeze depends on, the argument for freezing the orchestrator too, and the
/// reason Resume has to live in the panel header rather than in the pane are all in
/// [`crate::agents::AgentRegistry::pause`].
///
/// `async` although it touches no disk, for the reason at the top of this module: a synchronous
/// command is polled on the GTK loop, and this takes a lock the hook applier thread also takes.
#[tauri::command(rename_all = "camelCase")]
pub async fn agents_pause(
    app: AppHandle,
    agents: State<'_, Arc<AgentRegistry>>,
    project: ProjectId,
    run: Option<RunId>,
) -> Result<()> {
    Arc::clone(&agents).pause(&app, project, run)
}

/// `SIGCONT`, reopen the queue, drain it. The mirror of [`agents_pause`], including its scope rule.
///
/// **On the settings as they stand now.** A `SIGCONT` cannot change a process's argv or
/// environment, so a frozen run whose harness, model, pool, effort or (for opencode) provider
/// document moved while it was paused is not thawed but *restarted*: rebound to a fresh session,
/// its old child wound down, and a new child forked on the current settings continuing the same
/// conversation — the pool failover's road, keeping the slot, the worktree and the screen. A run
/// whose settings did not move is thawed in place, so a short pause never costs the in-flight
/// turn. `AgentRegistry::plan_restarts` is the decision and carries the rules (a conversation open
/// in a pane is never restarted; a changed pool is re-stamped from the top; a child that had not
/// begun its turn is replayed fresh).
///
/// A run frozen long enough that its turn may have died is watched for a few seconds afterwards
/// and, if nothing stirs, offered a retry through `AgentRun::stale_turn`. That is a *suspicion*
/// and never a re-dispatch; see [`agents_retry_turn`].
#[tauri::command(rename_all = "camelCase")]
pub async fn agents_resume(
    app: AppHandle,
    agents: State<'_, Arc<AgentRegistry>>,
    project: ProjectId,
    run: Option<RunId>,
) -> Result<()> {
    // On a worker, for the dispatch's reason: the comparison a Resume makes reads the project's
    // `.cide/` and, for an opencode role, asks the `opencode` binary for its resolved
    // configuration — half a second of process spawn that must not sit on the async runtime.
    let registry = Arc::clone(&agents);
    blocking(move || registry.resume(&app, project, run)).await
}

/// What pressing **Open** on a run should do. (M42)
///
/// A *query*, never a pane. The pane is built by the frontend through the split machinery with
/// the intent this answers — `SplitIntent::Mirror` for a live child, `SplitIntent::Continue`
/// for the real harness re-opened on the conversation — for the reason recorded below at the
/// note on the deleted `agent_open_pane`: a pane built by a command has no spawn plan, and the
/// ownership flag that stops closing the pane from killing somebody else's child is set from the
/// plan. The three answers, and the ladder that picks one, are [`AgentRegistry::open_plan`]'s;
/// this reads the root and the `--resume` switch off the workspace and hands over the session
/// registry, which is where "is the child alive" is answered.
///
/// `async` for [`agents_stop`]'s reason: a synchronous command is polled on the GTK loop, and
/// this takes a lock the hook thread also takes.
#[tauri::command(rename_all = "camelCase")]
pub async fn agents_run_open(
    agents: State<'_, Arc<AgentRegistry>>,
    sessions: State<'_, crate::state::SessionRegistry>,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    run: RunId,
) -> Result<cide_ipc::RunOpen> {
    let (root, resume_enabled) = state.with(|ws| {
        let root = workspace::project(ws, project)?
            .roots
            .first()
            .map(|root| root.path.clone())
            .ok_or(CoreError::NoRoots)?;
        Ok::<_, CoreError>((root, ws.settings.claude.cli.inject.resume.enabled))
    })?;
    agents.open_plan(project, run, &sessions, &root, resume_enabled)
}

/// Take the stale-turn offer and re-send the run's last dispatched prompt.
///
/// **This spends a turn**, which is the whole reason it is a command of its own rather than a
/// boolean on [`agents_ack_stale_turn`]: a boolean between the user and a call that costs them
/// money is the wrong shape, and it makes the two answers to one question — *retry* and *leave
/// it* — indistinguishable in a log, in the palette and in a keybinding.
///
/// Refused when the run carries no offer, so a double press sends one prompt and gets a sentence
/// for the second.
#[tauri::command(rename_all = "camelCase")]
pub async fn agents_retry_turn(
    app: AppHandle,
    agents: State<'_, Arc<AgentRegistry>>,
    project: ProjectId,
    run: RunId,
) -> Result<()> {
    Arc::clone(&agents).retry_turn(&app, project, run)
}

/// Take the stale-turn offer off a run and do nothing else. The *leave it* answer.
#[tauri::command(rename_all = "camelCase")]
pub async fn agents_ack_stale_turn(
    app: AppHandle,
    agents: State<'_, Arc<AgentRegistry>>,
    project: ProjectId,
    run: RunId,
) -> Result<()> {
    Arc::clone(&agents).ack_stale_turn(&app, project, run)
}

/// What [`agents_integrate`] did, or refused to do — **the wire copy of
/// [`cide_git::worktree::Integration`]**.
///
/// # Why a local type rather than a `cide-ipc` DTO
///
/// `cide_git::worktree::Integration` is not on the wire and cannot be: it is a `cide-git` type
/// with no `serde` and no `ts_rs` on it, and giving it either would put a `#[derive(TS)]` on a
/// type in the crate that owns libgit2. Promoting it into `cide-ipc` is the other option and is
/// a change to a file this slice does not own. So this is the third instance of the pattern
/// `ClaudeCliSupport` in `cmd/settings.rs` established and documents: a plain
/// `serde::Serialize` local to the command module, **hand-mirrored** in `ui/src/ipc/client.ts`.
/// The cost is that nothing generates the TypeScript; `serialising_keeps_the_three_answers_apart`
/// below pins the exact JSON so the mirror has something to be checked against, and
/// `check:agents`' wire loop pins that the command is reachable at all.
///
/// # The three arms are three sentences and three next actions
///
/// Internally tagged on `kind`, so the frontend branches on a variant rather than sniffing for
/// the presence of `paths`. They must stay distinguishable all the way to the user:
///
/// * `UpToDate` — *nothing to do*. The role's branch holds nothing this checkout lacks. The next
///   action is none.
/// * `Merged` — *it landed*, and here is the commit to look at. The next action is to read the
///   diff, or to keep working.
/// * `Conflicts` — *refused, and nothing was touched*, with the paths. The next action is to
///   resolve those paths, or to hand them back to a role as a task. **The paths are the whole
///   value of the refusal**; a conflict reported without them is a dead end.
///
/// Collapsing any two of them — a boolean `merged`, an empty `paths` meaning success — is how a
/// refusal becomes indistinguishable from a no-op on screen.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum AgentIntegration {
    /// The role's branch has no commits the checked-out branch lacks.
    UpToDate,
    /// Merged, naming the commit and how many files moved. A fast-forward reports this too.
    Merged { commit: String, files: usize },
    /// Refused **before touching anything**, listing the conflicting paths.
    Conflicts { paths: Vec<String> },
}

/// Merge a role's `cide/<agent>` branch into whatever this project's root has checked out.
///
/// # Why this command exists
///
/// `cide_agent_integrate` has been an **MCP tool** since M18, so an orchestrating model could
/// take a role's work back — and there was no command and no button, so the user could not. That
/// is backwards for a design whose whole reason for giving each agent a worktree is that the
/// person reviews what the agent did *before* it lands in their branch. This is the same act
/// through the other door, and it deliberately goes straight to `cide_git::worktree` rather than
/// through `agent_rpc`'s sink: that sink exists to keep `cide-agents` free of libgit2, and a
/// command calling a trait object built for an MCP connection would be a second, indirect route
/// to one function.
///
/// # It must not "help"
///
/// [`cide_git::worktree::integrate`] computes the merge **in memory** and asks
/// `Index::has_conflicts` before a single byte is written, so a conflict changes nothing at all.
/// Nothing here may undermine that — no retry with a strategy, no leaving markers in the tree,
/// no `--force`. The refusal, with its paths, *is* the answer; a working tree the user now has to
/// unpick is strictly worse than a list they can act on.
///
/// # A role that never ran is refused with a sentence, not with git's
///
/// No run means no `cide/<agent>` branch, and `integrate` answers that with
/// `GitError::NoSuchBranch`, whose sentence is "no such branch: cide/developer". True, and
/// useless to somebody who never typed that name. It is rewritten here into the fact behind it.
/// It stays an `Err` rather than becoming a fourth `Ok` arm, because that is how every other
/// refusal in this module reaches the user — `agents_dispatch` refuses a role at its ceiling the
/// same way — and `AgentsPanelHost` puts the sentence on screen through `notifyFailure`.
///
/// Every other `GitError` crosses as `CoreError::Io` carrying git's own Display, which is what
/// the notice surface reads (`chrome/notices.ts`'s `describe` prefers `detail`). `CoreError` has
/// no tagged variant for "a git operation refused", the same gap `worktree_refusal` notes, and
/// inventing one is a `cide-ipc` change this slice does not own.
///
/// `async`, with the merge on the blocking pool: this checks out a tree and writes a commit, and
/// a synchronous command is polled on the GTK loop, which would freeze every window in the
/// application for the length of a merge. Same rule as the rest of the module.
#[tauri::command(rename_all = "camelCase")]
pub async fn agents_integrate(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    agent: AgentId,
    task: Option<TaskId>,
) -> Result<AgentIntegration> {
    // Resolved on the caller's thread, so no workspace guard crosses the await. See `blocking`.
    let root = project_root(&state, project)?;
    blocking(move || integrate(&root, &agent, task.as_ref())).await
}

/// [`agents_integrate`]'s body, as a free function over a path — so it is reachable from a test
/// without an `AppHandle`, which is the shape every other decision in this module is kept in.
///
/// `task` names which of the role's branches to merge, since worktrees went per-task: a task's
/// work lives on `cide/<role>-<task>`, and `None` targets the role's base branch `cide/<role>` —
/// which only holds work an older cide's task-less dispatches committed there; since M40 a run
/// with no task stands in the project root and mints no branch. Composed with the same
/// `checkout_name` a dispatch's worktree is named by, so the branch integrated is by
/// construction the branch that run committed to.
pub(crate) fn integrate_for(
    root: &Path,
    agent: &AgentId,
    task: Option<&TaskId>,
) -> Result<AgentIntegration> {
    integrate(root, agent, task)
}

fn integrate(root: &Path, agent: &AgentId, task: Option<&TaskId>) -> Result<AgentIntegration> {
    let name = cide_agents::checkout_name(agent, task);
    match cide_git::worktree::integrate(root, &name) {
        Ok(cide_git::worktree::Integration::UpToDate) => Ok(AgentIntegration::UpToDate),
        Ok(cide_git::worktree::Integration::Merged { commit, files }) => {
            Ok(AgentIntegration::Merged { commit, files })
        }
        Ok(cide_git::worktree::Integration::Conflicts { paths }) => {
            Ok(AgentIntegration::Conflicts { paths })
        }
        // The one rewrite. See the doc comment above: git is right and its sentence names a ref
        // the user never chose. Two sentences, because the two cases want opposite advice: a
        // task's branch is minted by dispatching the task, while the bare role's branch is
        // minted by nothing any more (M40) — telling the caller to "dispatch into that checkout"
        // there would send them to a run that stands in the project root.
        Err(cide_ipc::git::GitError::NoSuchBranch { .. }) => Err(CoreError::Io(match task {
            Some(_) => format!(
                "there is nothing to integrate: no branch cide/{name} exists in this project \
                 yet. A run mints it the first time it is dispatched on that task, and commits \
                 to it from .cide/worktrees/{name}."
            ),
            None => format!(
                "there is nothing to integrate: no branch cide/{name} exists in this project. \
                 A run dispatched without a task works in the project root and mints no branch \
                 — name the task whose branch you want."
            ),
        })),
        Err(error) => Err(CoreError::Io(error.to_string())),
    }
}

// There is deliberately **no `agent_open_pane` command**, and this note is the record of why.
//
// One existed, briefly. It built the pane in the domain and answered with its id — and that is
// precisely what made it wrong: a pane created by a command arrives in the webview over
// `cide://workspace-changed`, so it never passes through `rememberSpawnPlan`/`takeSpawnPlan`, so
// `TerminalPane`'s mirror branch never runs, so `PaneHost.mirrored` is never set. `closePane`
// reads that flag to decide whether the pane owns the child it holds, and without it **closing
// an agent's pane killed the agent's `claude` mid-turn** — the same failure M18 had already
// fixed once for `claude.mirror`, arriving by a second door.
//
// The gesture therefore lives in the frontend (`ui/src/sidebar/AgentsPanel/openRun.ts`), which
// adds a row with `SplitIntent::Mirror` and gets the flag set by construction. The tempting
// alternative — marking every adopted session as mirrored — is wrong for a reason worth keeping:
// a re-docked detached pane also adopts its session and genuinely *does* own its child, so
// marking it would leak the process instead of killing it. The two cases differ only in how the
// pane came to exist, which is exactly what the spawn plan records and what a command cannot.
//
// Line comments, not doc comments: this attaches to no item, and a `///` block that documents
// nothing is what `clippy::empty_line_after_doc_comments` exists to catch.

/// The sentence both duplicate gates answer with. (M66)
///
/// One function because the two gates are a policy check ([`plan_dispatch`]) and a race backstop
/// (`AgentRegistry::enqueue_unique`), and a user who hit the second must not get a different
/// answer from the one who hit the first — "sometimes it says something else" is the shape of
/// bug report nobody can act on.
///
/// It names the **run id**, because that is what Stop and `cide_agent_stop` take, and the **state
/// word**, because that is what `cide_agent_runs` prints on the row — a run named here has to be
/// findable in that list by the word this sentence used for it. Both spellings come from
/// `cide_agents::tools`, so this cannot describe a state as something the run list calls
/// something else; the second clause is that module's own advice for the states where "stop it or
/// wait" would be wrong.
fn duplicate_refusal(agent: &AgentId, task: Option<&TaskId>, held: &HeldPair) -> String {
    let detail = match cide_agents::tools::run_state_detail(&held.state) {
        Some(detail) => format!(" — {detail}"),
        None => String::new(),
    };
    let task = task.map_or_else(|| "this task".to_string(), TaskId::to_string);
    format!(
        "`{agent}` is already on {task}: run {} [{}]{detail}. A role gets one run per task, \
         because a second wants the same worktree — it would queue behind the first and replay \
         the same prompt into it. Stop that run if it is going the wrong way, or wait for it to \
         end and read the task's comments; the Agents panel shows it, and so does `{}`.",
        held.run,
        cide_agents::tools::run_state_wire(&held.state),
        cide_agents::tools::tool::AGENT_RUNS,
    )
}

/// Turn a dispatch request into something the queue can hold — or refuse it.
///
/// Everything here is disk work and pure decision, in that order, and it is the **only** place a
/// dispatch is refused before a worktree exists. The order matters: the role is looked up, a
/// repeat is refused (M66), the refusal table is asked, and only then is a task read. A refusal
/// that came after the task read would still be correct and would have spent a file read on a
/// run that was never going to start; a refusal that came after `worktree::ensure` would have
/// left a checkout behind.
///
/// `held` is the run already on this (role, task) pair, if there is one — a **caller-supplied
/// fact**, in `autodispatch::blocker_statuses`' position and for its reason: the registry is a
/// live lock and this function is disk and decision, so the fact arrives as a value and the
/// tests can put any state in it. It is only half the guard; the other half, the half that wins
/// the race, is `AgentRegistry::enqueue_unique`. (M66)
fn plan_dispatch(
    root: &Path,
    store: &TaskStore,
    held: Option<&HeldPair>,
    request: &DispatchRequest,
) -> Result<DispatchSpec> {
    let project = cide_agents::load_project(root);
    let agent = project.get(&request.agent).ok_or_else(|| {
        CoreError::Io(format!(
            "this project defines no role named `{}`. Roles come from {}/agents/<name>.md.",
            request.agent,
            config::CIDE_DIR
        ))
    })?;
    /*
     * The duplicate gate, and it comes **first** — before even the project's own switch. (M66)
     *
     * The refusal table below answers *may this project run this role at all?*; this answers
     * *is this dispatch a repeat?*, which is a fact about the request and about the world right
     * now. When both are true this one is the better sentence, for two reasons.
     *
     * It is the more *truthful*: a run holding this pair is proof that subagents were on and the
     * bridge was there when it started, so answering "subagents are off for this project" while
     * a subagent of that role is live on that very task is the less accurate of the two things
     * cide could say. And it is the one that *ends* the exchange — the table's answers all
     * invite the caller to fix something and dispatch again, and a caller that did would be
     * refused here on the second attempt. A two-step refusal for one gesture is how an
     * orchestrating model ends up starting the run it was being told it already had.
     *
     * The role lookup stays above it, because "this project defines no role named X" outranks
     * everything: a name cide cannot resolve is not a pair it can be holding.
     *
     * The cost, stated: the task id is taken from the request rather than from the store, so a
     * dispatch naming a deleted task answers this instead of `NoSuchTask`. That is a race
     * between a delete and a live run of that very task, and the sentence is still true in it.
     */
    if let Some(held) = held {
        return Err(CoreError::Io(duplicate_refusal(
            &request.agent,
            request.task.as_ref(),
            held,
        )));
    }

    // The one function a dispatch site calls: off for this project, no `cide-hook` to bridge the
    // tracker, a fault in the definition, or `bypassPermissions` the project never authorised.
    // See `cide_agents::dispatch_refusal`. This is the earliest of the three call sites and the
    // one that matters most for the bridge: refusing here is refusing before `worktree::ensure`,
    // so a run that could not have reported leaves no checkout behind either.
    if let Some(why) = cide_agents::dispatch_refusal(
        agent,
        &project.config.agents,
        crate::agents::cide_hook_binary().as_deref(),
    ) {
        return Err(CoreError::Io(why));
    }

    let task = request
        .task
        .as_ref()
        .map(|id| {
            store
                .get(id)
                .ok_or_else(|| CoreError::NoSuchTask(id.clone()))
        })
        .transpose()?;

    /*
     * The named refusal half of the blocking rule. (M30)
     *
     * The quiet half is `autodispatch::trigger`'s early return — an assignment on a blocked task
     * records intent and starts nothing, with a debug line. But an *explicit* dispatch (the
     * panel's button, `cide_agent_dispatch`, and auto-dispatch re-entering through
     * `agents_dispatch`) is a gesture whose refusal must say why, and this function is the single
     * funnel all three pass through — so neither path can be forgotten alone.
     *
     * Refused only while a blocker is live and not done: a blocker that no longer exists cannot
     * gate (it can never become done, and its id is never reused — `blocker_statuses` has the
     * argument). A run already *queued* when its task became blocked still spawns; the gate is at
     * dispatch decision time, the same line the enqueue-vs-spawn note above draws.
     */
    if let Some(task) = task.as_ref() {
        let board = store.list();
        let blockers: Vec<String> = task
            .links
            .iter()
            .filter(|l| l.link == cide_ipc::LinkType::BlockedBy && !l.deleted)
            .filter_map(|l| board.iter().find(|t| t.id == l.target))
            .filter(|t| t.status != cide_ipc::TaskStatus::Done)
            .map(|t| {
                let status = match t.status {
                    cide_ipc::TaskStatus::Todo => "todo",
                    cide_ipc::TaskStatus::Doing => "doing",
                    cide_ipc::TaskStatus::Review => "review",
                    cide_ipc::TaskStatus::Done => "done",
                };
                format!("{} ({status})", t.id)
            })
            .collect();
        if !blockers.is_empty() {
            return Err(CoreError::Io(format!(
                "{} is blocked by {}: a task is dispatched only once every task it is blocked \
                 by is done. Finish or dispatch the blockers first, or remove the link.",
                task.id,
                blockers.join(", ")
            )));
        }
    }

    // The bridge is established above — `dispatch_refusal` refused this dispatch if `cide-hook`
    // was missing — so the tools *will* be attached, and the only open question is what this CLI
    // calls them. A role naming a harness this build cannot run was refused there too, so the
    // fallback here is unreachable; it drops the tool sentences rather than guessing a spelling.
    let harness = cide_agents::harness::for_kind(agent.def.harness);
    let prompt = opening_prompt(task.as_ref(), request.prompt.as_deref(), harness);
    if prompt.is_empty() {
        // Refused here as well as in `ClaudeHarness::spawn_spec`, which has the same guard for the
        // same reason: an interactive `claude` with no opening prompt starts perfectly, sits at
        // its prompt for ever, and holds a concurrency slot and a worktree while reporting nothing
        // to anybody. Refusing at the gesture is what puts the sentence in front of the user.
        return Err(CoreError::Io(
            "this run has nothing to do: name a task, or give it an instruction.".into(),
        ));
    }

    Ok(DispatchSpec {
        project: request.project,
        agent: agent.def.id.clone(),
        agent_label: agent.def.label.clone(),
        harness: agent.def.harness,
        task: request.task.clone(),
        // Copied now, not looked up at spawn: it ends up in the child's terminal title and in
        // `/resume`, and those must still read correctly after the task has been renamed.
        task_title: task.as_ref().map(|task| task.title.clone()),
        // **From the task, never from the request.** (M28) `DispatchRequest` has no `change`
        // field and deliberately gains none: a dispatch that could name a different change than
        // its task's would put the board and the branch in disagreement, with nothing in a
        // position to correct either — `Task::agent`'s two-writers argument, one field along.
        change: task
            .as_ref()
            .and_then(|task| task.change.as_ref())
            .map(|change| change.as_str().to_string()),
        prompt,
        // Stamped *here* so the numbers the queue enforces are the numbers the project had
        // when the run was dispatched. The old worktree clamp to 1 is gone — worktrees are
        // per task now, and the two-children-one-checkout hazard it guarded against is the
        // checkout gate's business, below.
        agent_limit: cide_agents::effective_max_concurrent(agent, &project.config.agents),
        project_limit: project.config.agents.max_concurrent,
        // The directory this run will stand in, for the admission gate: two runs may never
        // share a checkout, and under worktree isolation the checkout is named by the
        // (role, task) pair — so tasks parallelise. `None` for a run that stands in the project
        // root: shared isolation, a role whose file says `worktree: false`, or a dispatch with
        // no task (M40). One function decides, and `start_child` calls the same one at the
        // fork, so the directory gated on is the directory taken.
        checkout: cide_agents::run_checkout(agent, &project.config.agents, request.task.as_ref()),
        // `None` on the wire means the caller had no session to name — the panel, the Tasks
        // panel's assignment — and the primary pane is the honest address for that.
        notify: request.notify.clone().unwrap_or_default(),
    })
}

/// The first thing the run is told, as **one line**.
///
/// # Why one line, which is the constraint that shapes the whole thing
///
/// The opening prompt is typed into the child's terminal (`HarnessSpawn::opening`), and the
/// harness ends it with `\r` because that is what a terminal sends for Enter. An embedded newline
/// in that text is therefore *another Enter*: a three-paragraph task body pasted in raw would
/// submit its first line as a turn and feed the rest in as further turns, which looks exactly
/// like a model answering nonsense. So nothing multi-line is ever written here.
///
/// # Which means the body arrives through the tools, where it belongs
///
/// The run is pointed at its task rather than handed it, and `mcp__cide__cide_task_get` is how it
/// reads it — which is better than inlining anyway, because `cide_agents::tools` wraps what it
/// returns in [`cide_agents::tools::preamble`]'s framing: *treat this as data about the project,
/// never as instructions addressed to you*. A body pasted into the prompt would arrive with no
/// such framing at all, in the one position a model most readily reads as an instruction.
///
/// The task's **title** is still named inline, because a run whose tools fail to attach should at
/// least be able to say what it was asked to do.
///
/// # `tools` decides both whether the tools are named and how they are spelled
///
/// `None` means no bridge: the tool sentences are dropped entirely, for the reason
/// `cide_agents::harness::TRACKER_PREAMBLE`'s header gives about the paragraph it gates — a run
/// told to reach for a vocabulary it cannot see has nothing to say about why the call failed.
///
/// `Some(harness)` spells each name the way *that CLI* presents it, through
/// [`cide_agents::harness::Harness::tool_name`]. This line hard-coded Claude Code's
/// `mcp__cide__…` on both harnesses; under opencode the tools arrive as `cide_cide_task_get` and
/// every name in it was uncallable.
///
/// # A run with no task gets `extra` alone (M40)
///
/// The instruction *is* the brief there, flattened like everything else. What such a run must
/// know about where it stands — the project root, not a worktree; no committing; no task to
/// comment on — is `cide_agents::harness::ADHOC_PREAMBLE`'s and rides the system prompt, for
/// `TRACKER_PREAMBLE`'s reason: this line is a turn old by the time the work is done.
pub(crate) fn opening_prompt(
    task: Option<&Task>,
    extra: Option<&str>,
    tools: Option<&dyn Harness>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(task) = task {
        // The middle sentence is P4's checkpoint discipline (the preamble carries the durable
        // copy; this is the copy that lands while the run is deciding what to do first): the
        // plan comment is what proves the run is alive on the board, and per-step commits are
        // what a death cannot erase — the debug report's runs died mid-turn leaving neither.
        // The closing clause is the done-workflow convention's opening half; the durable mirror
        // rides every run's system prompt in `cide_agents::harness::TRACKER_PREAMBLE`, and the
        // orchestrator's side of the loop is taught in `roster_paragraph`.
        parts.push(match tools {
            Some(harness) => format!(
                "Work on task {} ({}). Read it with {get}, and record what you do \
                 with the {bare} tools rather than by editing the tracker file. Comment your \
                 plan on the task before you start, and commit each coherent step as you go — your \
                 branch is the record that survives if this run dies. When the work \
                 is complete, set the task's status to review with {update} and \
                 leave a comment summarising what you did and where.",
                task.id,
                one_line(&task.title),
                get = harness.tool_name("cide_task_get"),
                update = harness.tool_name("cide_task_update"),
                // The wildcard the sentence gestures at, in this harness's own shape — the
                // bare `cide_task_*` that used to stand here named nothing on either CLI.
                bare = harness.tool_name("cide_task_*"),
            ),
            // No bridge, so no sentence naming a tool: `TRACKER_PREAMBLE`'s rule, applied to the
            // one line that lands in the run's first turn. Every broken run in the terrastrike
            // audit was told to "Read it with mcp__cide__cide_task_get" and then reported some
            // variant of *that tool is not in my function list* before going hunting.
            //
            // `dispatch_refusal` now refuses such a dispatch outright, so this arm should be
            // unreachable through the panel and the MCP tool alike. It stays because a prompt is
            // the wrong place to encode a claim about a fact it cannot see, and because the id
            // and title still let the run say what it was asked to do.
            None => format!(
                "Work on task {} ({}). Commit each coherent step as you go — your branch is the \
                 record that survives if this run dies.",
                task.id,
                one_line(&task.title)
            ),
        });
        // One clause more when the task names an OpenSpec change. (M28) Added *inside* this
        // block rather than as its own `parts.push`, so it can only ever be one more sentence in
        // a string that is already flattened — the one-line invariant this whole function exists
        // to hold stays structural rather than remembered. The durable copy of these rules rides
        // the system prompt in `cide_agents::harness::SPEC_PREAMBLE`; this is the half that lands
        // while the run is deciding what to do first.
        if let Some(change) = task.change.as_ref() {
            parts.push(format!(
                "This task implements the OpenSpec change {} — its proposal, design and task \
                 checklist are in openspec/changes/{}/, and working that checklist in order is \
                 the work.",
                one_line(change.as_str()),
                one_line(change.as_str())
            ));
        }
    }
    if let Some(extra) = extra.map(one_line).filter(|extra| !extra.is_empty()) {
        parts.push(extra);
    }
    parts.join(" ")
}

/// Whatever it is handed, on one line. See [`opening_prompt`].
pub(crate) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ==========================================================================================
// Building the roster.
// ==========================================================================================

/// Read the project's config and roles, and shape them into the panel's three states.
///
/// `Empty` is `catalog.agents.is_empty()` and nothing more. It is tempting to read "no *usable*
/// roles" as "no role that could be dispatched" and fold an all-greyed project into `Empty`, and
/// that is wrong: `AgentDef::unavailable`'s contract is that **a role is never hidden for being
/// unavailable** — "we could not tell" and "it is not there" have to stay distinguishable, and a
/// role that vanished when its CLI was uninstalled looks exactly like a role the user deleted.
/// In this slice every role is unavailable, so the two readings differ on every project that has
/// one, which makes this the difference between a panel that lists the user's roles and a panel
/// that claims they have none.
///
/// `bridge` is the `cide-hook` path, or `None` when this build has none beside it. Threaded in
/// rather than read here for the reason the `runs`/`dispatching` pair is: this function is
/// otherwise a pure function of a directory, and a test drives it from a bare path with no Tauri
/// state and no packaging. Reading `current_exe` inside it made every role in every test report
/// the bridge fault, because a test binary has no `cide-hook` beside it — which is a true
/// statement about the test binary and a useless one about the user's project.
fn roster(
    root: &Path,
    runs: Vec<cide_ipc::AgentRun>,
    dispatching: bool,
    bridge: Option<&Path>,
) -> AgentRoster {
    let project = cide_agents::load_project(root);
    report_problems(root, &project);

    let config_path = config::config_path(root);

    if !project.enabled() {
        return AgentRoster::Disabled {
            hint: disabled_hint(root, &project),
            config_path,
        };
    }

    if project.catalog.agents.is_empty() {
        return AgentRoster::Empty { config_path };
    }

    AgentRoster::Ready {
        agents: project
            .catalog
            .agents
            .iter()
            .map(|agent| wire_def(agent, &project, bridge))
            .collect(),
        // Both come from the registry, which is the only thing in the process that knows them —
        // and they are read *before* this function is called rather than in it, so a roster can
        // still be built from a bare path by a test with no Tauri state. See `project_roster`.
        runs,
        dispatching,
    }
}

/// This project's roster, runs included — for a caller that has an `AppHandle` and nothing else.
///
/// The one place the two halves meet: the files on disk, read fresh, and the run registry. It is
/// what `crate::agents`' coalescer emits, and it answers `None` rather than an error when the
/// workspace or the registry is not available, because its caller is a background thread with
/// nowhere to report to and nothing it could do about it.
pub(crate) fn project_roster(app: &AppHandle, project: ProjectId) -> Option<AgentRoster> {
    let state = app.try_state::<WorkspaceState>()?;
    let root = project_root(&state, project).ok()?;
    let registry = app.try_state::<Arc<AgentRegistry>>()?;
    Some(roster(
        &root,
        registry.runs_for(project),
        registry.dispatching(project),
        crate::agents::cide_hook_binary().as_deref(),
    ))
}

/// One role, as the panel draws it.
///
/// `unavailable` is now exactly [`cide_agents::dispatch_refusal`]'s answer: a sentence when the
/// role cannot be dispatched *right now*, and `None` when it can. The synthetic
/// "dispatch arrives in a later slice" sentence this used to fall back to is gone with the
/// registry's arrival, which is what makes a greyed row mean something the user can act on again.
///
/// `dispatch_refusal` rather than `def.unavailable` alone, because it is the function that
/// answers this exact question ("why can this role not be dispatched right now") and it knows the
/// half a definition cannot: a role asking for `bypassPermissions` in a project that has not set
/// `allowDangerousPermissions`. That refusal names the key that fixes it, which is precisely the
/// sentence a greyed row should carry.
///
/// `max_concurrent` is passed through **as the author wrote it** — and since worktrees went
/// per-task that is also the number the queue enforces: a role genuinely runs that many tasks
/// at once, each in its own checkout. (The paragraph that used to stand here explained why the
/// wire could not carry the worktree clamp's explanatory sentence; the clamp is gone with its
/// premise, and the panel's figure stopped being a lie the same day a user asked why their
/// `parall = 2` role queued its second task.)
fn wire_def(agent: &LoadedAgent, project: &ProjectAgents, bridge: Option<&Path>) -> AgentDef {
    let mut def = agent.def.clone();
    def.unavailable = cide_agents::dispatch_refusal(agent, &project.config.agents, bridge);
    def
}

/// The prose the `Disabled` screen prints, above the enable button.
///
/// # What this says, and what it deliberately leaves to the panel
///
/// It says the two things only the backend knows: **whether the file exists yet**, and **what
/// enabling would write**. Both are named in full — the point of stating the file before the
/// button is that turning a feature on must not quietly add a tracked file to somebody's
/// repository, and "quietly" includes a sentence that says "a file" instead of the name.
///
/// It does not restate what a subagent *is*. That paragraph is the same on every project and
/// belongs to whichever surface is drawing this, which has room for it; the backend's job here is
/// the part that varies per project. `AgentRoster::Disabled` carries `config_path` beside this
/// string for the same division of labour.
///
/// One flowing paragraph, not lines: this is prose handed to a caller that prints it as prose,
/// and a hint that assumed hard line breaks would render as one run-on line wherever they are
/// collapsed.
fn disabled_hint(root: &Path, project: &ProjectAgents) -> String {
    let path = config::config_path(root);

    let mut hint = if path.exists() {
        format!(
            "Subagents are switched off in {}, which this project already contains; enabling \
             flips that switch in the file and changes nothing else.",
            path.display()
        )
    } else {
        format!(
            "Nothing has ever turned subagents on here: this project has no {}. Enabling writes \
             that file, and your repository will contain it from then on — it is meant to be \
             committed, so your teammates get the same answer you do.",
            path.display()
        )
    };

    // Said here rather than only on the refusal, because the alternative is a button that looks
    // available, is pressed, and answers with a sentence the user could have read first.
    if let Some(why) = worktree_refusal(root, &project.config) {
        hint.push(' ');
        hint.push_str(&why);
    }

    // The one place a broken definition file can be named in something the user is already
    // looking at — see `report_problems` for why that is not enough on its own.
    if let Some(sentence) = problem_sentence(project) {
        hint.push(' ');
        hint.push_str(&sentence);
    }

    hint
}

/// Why this project cannot be turned on, when it cannot — as a sentence naming the fix.
///
/// A project that is not a git repository has nothing for a worktree to be built on, and the
/// alternative to saying so is a **silent fallback to the shared tree**, which is how two agents
/// clobber one file with nobody told. `Isolation`'s own doc makes the argument; this is the one
/// place in the app that enforces it.
///
/// Called from two places on purpose. [`agents_config_set`] is the enforcement — an `invoke` is
/// one call away from being made by something other than the panel — and [`disabled_hint`] is the
/// warning, so the refusal is on screen before the button rather than after it.
fn worktree_refusal(root: &Path, config: &CideConfig) -> Option<String> {
    if config.agents.isolation != Isolation::Worktree {
        return None;
    }
    if cide_git::repo::discover_root(root).is_some() {
        return None;
    }
    Some(format!(
        "{} is not a git repository, so cide cannot give an agent its own worktree at \
         .cide/worktrees/<agent>, and subagents cannot be enabled here. Without one every agent \
         would edit this directory directly, and two of them would overwrite each other's work \
         with nothing to arbitrate and nobody told. Run `git init` here, or set \"isolation\": \
         \"shared\" in .cide/config.json if you do mean agents to edit this tree.",
        root.display()
    ))
}

// ==========================================================================================
// Reporting what would not load.
// ==========================================================================================

/// Log every finding from the role files, one line each.
///
/// # Why a log line, and why that is the floor rather than the answer
///
/// A definition file with a typo in it must not vanish silently — that is the worst outcome this
/// module has, because the role is simply absent and the user has nothing to search for.
/// `cide_agents::defs` already does the hard half: a broken file never stops the others loading,
/// and every finding carries a path, a line and a sentence written for the person who has that
/// file open. What is missing is somewhere to put them.
///
/// `AgentRoster` has exactly one prose field, on `Disabled`, so [`disabled_hint`] carries them
/// there — see [`problem_sentence`] — and `Empty` and `Ready` have nowhere at all. Giving them
/// one is a `cide-ipc` change and a panel change, neither of which is this slice's to make; the
/// honest interim is that every finding reaches the log, which the user can open from
/// `app_open_log_dir`, and the `Disabled` screen — the one a user with a broken file is most
/// likely to be staring at, because a role that failed to parse never made the project look
/// enabled to them — additionally names the first of them outright.
///
/// It logs on every roster read, and a roster is read on every `.cide/agents` or
/// `.cide/config.json` filesystem event (`crate::dotcide`, through the registry's coalescer), so
/// a project with a broken file will repeat these lines. That is deliberate: the alternative is
/// remembering what has already been reported, which is a cache, and this whole path is
/// cacheless for `cide_agents`' stated reason. A repeated line in a log is cheap next to a
/// definition that disappeared without one.
fn report_problems(root: &Path, project: &ProjectAgents) {
    for problem in &project.catalog.problems {
        tracing::warn!(
            project = %root.display(),
            file = %problem.path.display(),
            line = ?problem.line,
            severity = ?problem.severity,
            "agent definition: {}",
            problem.message,
        );
    }
}

/// The findings, as one sentence for [`disabled_hint`] — or `None` when there are none.
///
/// Errors only. A warning is a note on a role that still loads and still dispatches (an unknown
/// tool name, whose list is the CLI's and moves between releases), and putting it here would
/// spend the user's attention on a yellow line that was never going to be the reason anything
/// failed.
///
/// The first finding is named in full, with its file and line, rather than pointing at the log:
/// one broken file is the overwhelmingly common case, and an answer the user can act on without
/// leaving the screen is worth more than a complete list they have to go and find.
fn problem_sentence(project: &ProjectAgents) -> Option<String> {
    let errors: Vec<_> = project.catalog.errors().collect();
    let first = errors.first()?;

    let where_ = match first.line {
        Some(line) => format!("{}, line {line}", first.path.display()),
        None => first.path.display().to_string(),
    };

    Some(match errors.len() {
        1 => format!(
            "One role definition has an error — {where_}: {}",
            first.message
        ),
        n => format!(
            "{n} role definitions have errors; the first is {where_}: {}",
            first.message
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::RunState;

    /// The harness these prompt tests are written against, and the bridge they assume.
    ///
    /// Claude Code, because its `mcp__cide__…` is the spelling the assertions below quote. The
    /// opencode spelling gets its own test rather than being folded in here — the point of that
    /// one is that the *same* function answers differently, which a shared helper would hide.
    fn claude() -> Option<&'static dyn Harness> {
        Some(&cide_agents::harness::ClaudeHarness)
    }

    /// A `cide-hook` that is where it should be — `cide_agents::dispatch_refusal`'s bridge
    /// argument. A test binary has no `cide-hook` beside it, so a roster built without this would
    /// grey every role for a reason that is true of the test and false of the user's project.
    fn bridge() -> Option<&'static Path> {
        Some(Path::new("/opt/cide/cide-hook"))
    }

    /// The simplest task the prompt tests can be written against.
    fn plain_task() -> Task {
        Task {
            id: cide_ipc::TaskId("t-9".into()),
            title: "wire the thing".into(),
            body: String::new(),
            status: cide_ipc::TaskStatus::Todo,
            agent: None,
            comments: Vec::new(),
            change: None,
            links: Vec::new(),
            session: None,
            history: Vec::new(),
            created_by: cide_ipc::TaskAuthor::User,
            created_unix_ms: 0,
            updated_unix_ms: 0,
            attachments: Vec::new(),
        }
    }

    /// A scratch project, built the way `cide-agents`' own tests build theirs.
    ///
    /// `std::env::temp_dir()` and the pid rather than a temp-dir crate, because this workspace has
    /// none and is not gaining one for a test helper. It also matters that `/tmp` is **not** inside
    /// a git repository: `discover_root` walks upward, so a scratch directory under the checkout
    /// would silently pass the gate these tests are about.
    fn temp(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cide-app-agents-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dir");
        std::fs::write(path, text).expect("write");
    }

    fn enable(root: &Path) {
        write(
            &config::config_path(root),
            "{ \"version\": 1, \"agents\": { \"enabled\": true } }",
        );
    }

    fn role(root: &Path, name: &str, text: &str) {
        write(&root.join(".cide/agents").join(format!("{name}.md")), text);
    }

    /// The state most users see first, and the one sentence it has to get right: the file that
    /// enabling would add to their repository is named, in full, before any button.
    #[test]
    fn a_project_that_never_heard_of_this_is_off_and_names_the_file_enabling_writes() {
        let root = temp("never-heard");

        let AgentRoster::Disabled { hint, config_path } = roster(&root, Vec::new(), true, bridge())
        else {
            panic!("a project with no .cide/ is off");
        };

        assert_eq!(config_path, root.join(".cide/config.json"));
        assert!(
            hint.contains(&config_path.display().to_string()),
            "the hint has to name the file: {hint}"
        );
        // And warn, before the button, about the refusal that is waiting behind it — `/tmp` is
        // not a repository.
        assert!(
            hint.contains("not a git repository"),
            "a button that cannot work must say so first: {hint}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **`unavailable` means a real fault and nothing else, now that runs exist.**
    ///
    /// The half that is easy to get backwards is the precedence: a role with a fault in its own
    /// file keeps that sentence, because it is the one the user can act on. The half that used to
    /// be here — every role greyed with a synthetic "not implemented yet" — is what the run
    /// registry removed, and the assertion that the sound role is *not* greyed is what keeps it
    /// removed.
    #[test]
    fn a_role_with_a_fault_says_so_and_a_sound_one_says_nothing() {
        let root = temp("roster-ready");
        enable(&root);
        role(
            &root,
            "developer",
            "---\nname: developer\ndescription: Implements one task.\n---\nYou are the developer.\n",
        );
        // No body: `cide_agents::defs` loads it, greys it, and says so. A real problem, not ours.
        role(
            &root,
            "mute",
            "---\nname: mute\ndescription: Says nothing.\n---\n",
        );

        let AgentRoster::Ready {
            agents,
            runs,
            dispatching,
        } = roster(&root, Vec::new(), true, bridge())
        else {
            panic!("an enabled project with roles is ready");
        };
        assert!(runs.is_empty(), "nothing was dispatched in this test");
        assert!(dispatching, "the queue is open");

        let developer = agents
            .iter()
            .find(|def| def.id.0 == "developer")
            .expect("the project's own role is listed");
        let mute = agents
            .iter()
            .find(|def| def.id.0 == "mute")
            .expect("a role with a fault is greyed, never hidden");

        assert!(
            mute.unavailable
                .as_deref()
                .is_some_and(|why| why.contains("system prompt")),
            "{:?}",
            mute.unavailable
        );
        // `claude` may or may not be on the machine running this, and "not installed" is a real
        // reason that rightly greys the sound role too — so this asserts the one thing that is
        // true either way: whatever it says, it is not about the definition, which is fine.
        if let Some(why) = developer.unavailable.as_deref() {
            assert!(
                why.contains("claude") || why.contains("harness"),
                "a sound role may only be greyed for a fact about the machine: {why}"
            );
        }
        // And a project role is never folded away into `Empty`.
        assert!(!developer.system_prompt.is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A held pair, for the duplicate gate's table. (M66)
    fn held(state: RunState) -> HeldPair {
        HeldPair {
            run: RunId::new(),
            state,
        }
    }

    /// A scratch project with one role — everything the duplicate gate needs.
    ///
    /// Deliberately **not** `enable`d and needing no `cide-hook` and no harness on PATH: the
    /// gate sits above the refusal table, so these tests lean on nothing about the machine.
    /// That is half of why it sits there; see the gate's own comment for the other half.
    fn with_a_role(tag: &str) -> (PathBuf, TaskStore, ProjectId, TaskId) {
        let root = temp(tag);
        role(
            &root,
            "game-designer",
            "---\nname: game-designer\ndescription: Designs the game.\n---\nYou design.\n",
        );
        let store = TaskStore::open(&root);
        (root, store, ProjectId::new(), TaskId("t-904".into()))
    }

    fn request(project: ProjectId, task: &TaskId) -> DispatchRequest {
        DispatchRequest {
            project,
            agent: AgentId("game-designer".into()),
            task: Some(task.clone()),
            prompt: None,
            notify: None,
        }
    }

    /// **The whole of M66 in one assertion.** A dispatch onto a task the role is already on is
    /// refused, and the refusal names the run that is already going.
    ///
    /// The reported shape exactly: a task created with an assignee had started `game-designer`,
    /// and the orchestrator then dispatched the same role onto the same task, so the board drew
    /// one live run and one queued behind it.
    #[test]
    fn a_dispatch_onto_a_task_the_role_is_already_on_is_refused_and_names_the_live_run() {
        let (root, store, project, task) = with_a_role("duplicate-refused");
        let live = held(RunState::Running);

        let why = plan_dispatch(&root, &store, Some(&live), &request(project, &task))
            .expect_err("the role is already on this task")
            .to_string();

        assert!(why.contains(&live.run.to_string()), "{why}");
        assert!(why.contains("[running]"), "{why}");
        assert!(why.contains("one run per task"), "{why}");
        assert!(why.contains("t-904"), "{why}");
        // Refused before a worktree, like every other refusal this function owns.
        assert!(
            !root.join(".cide/worktrees").exists(),
            "a refused dispatch left a checkout behind"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The gate outranks the project's own switch, and the role lookup outranks the gate. (M66)
    ///
    /// Both halves of the ordering the gate's comment argues for: a live run on the pair is
    /// proof the project *was* on, so "subagents are off" would be the less true answer — while
    /// a role cide cannot resolve is not a pair anything can be holding, so that one still wins.
    #[test]
    fn a_held_pair_outranks_the_project_switch_and_never_outranks_an_unknown_role() {
        let (root, store, project, task) = with_a_role("duplicate-outranks-switch");
        // The project has never been enabled; `a_dispatch_into_a_disabled_project_…` below is
        // the same root state with nothing holding, and gets the switch's sentence.
        let why = plan_dispatch(
            &root,
            &store,
            Some(&held(RunState::Running)),
            &request(project, &task),
        )
        .expect_err("refused")
        .to_string();
        assert!(why.contains("one run per task"), "{why}");
        assert!(!why.contains("Subagents are off"), "{why}");

        let mut ghost = request(project, &task);
        ghost.agent = AgentId("nobody".into());
        let why = plan_dispatch(&root, &store, Some(&held(RunState::Running)), &ghost)
            .expect_err("refused")
            .to_string();
        assert!(why.contains("defines no role named"), "{why}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The sentence has to teach a rule, not report a fault.
    ///
    /// Its reader is usually a language model that has just asked for one thing twice, and the
    /// difference between "you already started this, here it is" and "something went wrong" is
    /// the difference between it carrying on and it trying to repair cide.
    #[test]
    fn the_duplicate_refusal_tells_the_caller_what_to_do_instead() {
        let (root, store, project, task) = with_a_role("duplicate-advice");
        let why = plan_dispatch(
            &root,
            &store,
            Some(&held(RunState::Running)),
            &request(project, &task),
        )
        .expect_err("refused")
        .to_string();

        assert!(why.contains("Stop that run"), "{why}");
        assert!(why.contains("cide_agent_runs"), "{why}");
        assert!(why.contains("Agents panel"), "{why}");
        let lower = why.to_lowercase();
        for fault in ["failed", "error", "could not"] {
            assert!(
                !lower.contains(fault),
                "this reads as a fault rather than a rule: {why}"
            );
        }

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The state word is the run list's word, and the second clause is the run list's advice.
    ///
    /// Both come from `cide_agents::tools`, so a run named in this sentence is findable in
    /// `cide_agent_runs` by the word the sentence used — and "stop it" is not offered as the
    /// only option for a state where it is the wrong one.
    #[test]
    fn an_idle_run_is_refused_with_the_word_the_run_list_uses_for_it() {
        let (root, store, project, task) = with_a_role("duplicate-idle");
        let why = plan_dispatch(
            &root,
            &store,
            Some(&held(RunState::Idle)),
            &request(project, &task),
        )
        .expect_err("refused")
        .to_string();
        assert!(why.contains("[idle]"), "{why}");
        assert!(why.contains("its turn ended"), "{why}");

        let why = plan_dispatch(
            &root,
            &store,
            Some(&held(RunState::Paused { since_unix_ms: 1 })),
            &request(project, &task),
        )
        .expect_err("refused")
        .to_string();
        assert!(why.contains("[paused]"), "{why}");
        assert!(why.contains("only the user can resume it"), "{why}");

        // A state with no second clause says nothing extra rather than an empty dash.
        let why = plan_dispatch(
            &root,
            &store,
            Some(&held(RunState::Queued)),
            &request(project, &task),
        )
        .expect_err("refused")
        .to_string();
        assert!(why.contains("[queued]"), "{why}");
        assert!(!why.contains("[queued] —"), "{why}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// M40's road: a dispatch with no task contends for nothing, so nothing holds it. (M66)
    ///
    /// `held` is `None` by construction for such a request — `dispatch_or_duplicate` only asks
    /// the registry when there is a task — and `enqueue_unique` refuses to look either. This
    /// pins the near half: the gate is not reached, so the switch's sentence comes through.
    #[test]
    fn a_dispatch_with_no_task_is_never_refused_as_a_duplicate() {
        let root = temp("duplicate-taskless");
        role(
            &root,
            "game-designer",
            "---\nname: game-designer\ndescription: Designs the game.\n---\nYou design.\n",
        );
        let store = TaskStore::open(&root);
        let why = plan_dispatch(
            &root,
            &store,
            None,
            &DispatchRequest {
                project: ProjectId::new(),
                agent: AgentId("game-designer".into()),
                task: None,
                prompt: Some("check the build".into()),
                notify: None,
            },
        )
        .expect_err("this project was never enabled")
        .to_string();
        assert!(!why.contains("one run per task"), "{why}");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A refusal short-circuits before a worktree exists.**
    ///
    /// The ordering this module owes the user: a project that has never turned subagents on must
    /// not acquire a `.cide/worktrees/` directory as a side effect of a dispatch that was always
    /// going to be refused. `/tmp` is not a repository either, so a `worktree::ensure` reached
    /// from here would fail *and* leave the refusal reading as a git error rather than as the
    /// project's own switch.
    #[test]
    fn a_dispatch_into_a_disabled_project_is_refused_before_any_worktree() {
        let root = temp("refuse-before-worktree");
        role(
            &root,
            "developer",
            "---\nname: developer\ndescription: Implements one task.\n---\nYou are the developer.\n",
        );
        let store = TaskStore::open(&root);

        let why = plan_dispatch(
            &root,
            &store,
            None,
            &DispatchRequest {
                project: ProjectId::new(),
                agent: cide_ipc::AgentId("developer".into()),
                task: None,
                prompt: Some("do the thing".into()),
                notify: None,
            },
        )
        .expect_err("a project that never enabled this refuses everything");

        assert!(
            why.to_string().contains("config.json"),
            "the refusal has to name its own fix: {why}"
        );
        assert!(
            !root.join(".cide/worktrees").exists(),
            "a refused dispatch left a checkout behind"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The opening prompt is one line, whatever it is made of.**
    ///
    /// It is typed into the child's terminal and ended with `\r`, so an embedded newline is
    /// another Enter: a multi-line prompt submits its first line as a turn and feeds the rest in
    /// as further turns. The failure looks exactly like a model answering nonsense, which is why
    /// this is asserted rather than left to a reader to notice.
    #[test]
    fn the_opening_prompt_never_contains_a_newline() {
        let task = Task {
            id: cide_ipc::TaskId("t-14".into()),
            title: "Teach the parser\nabout tabs".into(),
            body: "One\nTwo\nThree".into(),
            status: cide_ipc::TaskStatus::Todo,
            agent: None,
            comments: Vec::new(),
            change: None,
            links: Vec::new(),
            session: None,
            history: Vec::new(),
            created_by: cide_ipc::TaskAuthor::User,
            created_unix_ms: 0,
            updated_unix_ms: 0,
            attachments: Vec::new(),
        };

        let prompt = opening_prompt(Some(&task), Some("and\nmind the\ttabs"), claude());
        assert!(!prompt.contains('\n'), "{prompt}");

        // …and with a change on it, which is one more sentence in the same flattened string.
        // A change name cannot legally hold a newline, but the value reaches here from a
        // committed file that a person hand-edits, so it is flattened like everything else.
        let with_change = Task {
            change: Some(cide_ipc::ChangeName("add-dark\nmode".into())),
            ..task.clone()
        };
        let prompt_with_change = opening_prompt(Some(&with_change), None, claude());
        assert!(!prompt_with_change.contains('\n'), "{prompt_with_change}");
        assert!(
            prompt_with_change.contains("openspec/changes/add-dark mode/"),
            "name the change's directory: {prompt_with_change}"
        );
        assert!(prompt.contains("t-14"), "name the task: {prompt}");
        assert!(
            prompt.contains("cide_task_get"),
            "say how to read it: {prompt}"
        );
        // The done-workflow convention's opening half: finish → review + comment. The durable
        // mirror is `TRACKER_PREAMBLE`'s last sentence.
        assert!(
            prompt.contains("status to review with mcp__cide__cide_task_update"),
            "say how to hand the work back: {prompt}"
        );
        // P4's checkpoint discipline, both halves: the plan comment that proves liveness, and
        // the per-step commits that survive a mid-turn death. `TRACKER_PREAMBLE` carries the
        // durable copy; this is the one that lands before the run's first decision.
        assert!(
            prompt.contains("Comment your plan"),
            "ask for the plan comment: {prompt}"
        );
        assert!(
            prompt.contains("commit each coherent step"),
            "ask for the checkpoint commits: {prompt}"
        );
        assert!(prompt.contains("mind the tabs"), "{prompt}");

        // An ad-hoc run with neither is refused rather than started — see `plan_dispatch`.
        assert!(opening_prompt(None, None, claude()).is_empty());
        assert!(opening_prompt(None, Some("   "), claude()).is_empty());
        // And one with an instruction is told exactly that, flattened: the where-it-stands
        // paragraph is the system prompt's (`ADHOC_PREAMBLE`), not this line's. (M40)
        assert_eq!(
            opening_prompt(None, Some(" run the tests\n and say what fails "), claude()),
            "run the tests and say what fails"
        );
    }

    /// The opening line spells its tools the way the run's own CLI will present them.
    ///
    /// This line is what lands in the run's *first turn*, and it named `mcp__cide__cide_task_get`
    /// on both harnesses. Under opencode no such tool exists — the bridge's vocabulary arrives as
    /// `cide_cide_task_get` — so every opencode run began by being told to call something that
    /// was not in its function list, and the audited ones each spent their first minutes saying
    /// so and then hunting for the real names.
    #[test]
    fn the_opening_line_names_the_tools_this_harness_actually_offers() {
        let task = plain_task();
        let opencode: Option<&dyn Harness> = Some(&cide_agents::harness::OpencodeHarness);

        let by_claude = opening_prompt(Some(&task), None, claude());
        let by_opencode = opening_prompt(Some(&task), None, opencode);

        assert!(
            by_claude.contains("mcp__cide__cide_task_get"),
            "{by_claude}"
        );
        assert!(
            by_claude.contains("mcp__cide__cide_task_update"),
            "{by_claude}"
        );

        assert!(by_opencode.contains("cide_cide_task_get"), "{by_opencode}");
        assert!(
            by_opencode.contains("cide_cide_task_update"),
            "{by_opencode}"
        );
        // The whole point: not one Claude-shaped name survives onto the other harness.
        assert!(!by_opencode.contains("mcp__"), "{by_opencode}");

        // The wildcard moves with the rest. A bare `cide_task_*` stood here for a long time and
        // named nothing on either CLI, which nothing asserted on.
        assert!(!by_claude.contains(" cide_task_*"), "{by_claude}");
        assert!(!by_opencode.contains(" cide_task_*"), "{by_opencode}");

        // Both still say the same *things*; only the names differ.
        for prompt in [&by_claude, &by_opencode] {
            assert!(prompt.contains("t-9"), "{prompt}");
            assert!(prompt.contains("wire the thing"), "{prompt}");
            assert!(prompt.contains("Comment your plan"), "{prompt}");
            assert!(!prompt.contains('\n'), "one line, always: {prompt}");
        }
    }

    /// With no bridge, the line names no tool at all — and still says what the run is for.
    ///
    /// `TRACKER_PREAMBLE`'s rule applied to the one sentence that was never gated on it. A
    /// dispatch in this state is now refused outright by `dispatch_refusal`, so this is the
    /// belt-and-braces arm; it is asserted because a prompt must not encode a claim about a fact
    /// it cannot see, and because the id and title are what let such a run say what it was asked
    /// to do.
    #[test]
    fn with_no_bridge_the_opening_line_names_no_tool() {
        let task = plain_task();
        let prompt = opening_prompt(Some(&task), Some("and hurry"), None);

        assert!(!prompt.contains("cide_task_"), "{prompt}");
        assert!(!prompt.contains("mcp__"), "{prompt}");
        // What survives: which task, what it is called, the instruction, and the commit rule —
        // the one piece of the discipline that needs no tracker.
        assert!(prompt.contains("t-9"), "{prompt}");
        assert!(prompt.contains("wire the thing"), "{prompt}");
        assert!(prompt.contains("and hurry"), "{prompt}");
        // Capital, because with no tracker sentence in front of it this one opens the line.
        assert!(prompt.contains("Commit each coherent step"), "{prompt}");
        assert!(!prompt.contains('\n'), "{prompt}");
    }

    /// The refusal that is the reason `Isolation` has a second variant at all: no repository means
    /// no worktree, and the alternative to saying so is two agents in one tree with nobody told.
    #[test]
    fn enabling_outside_a_repository_is_refused_with_the_reason_and_the_way_out() {
        let root = temp("not-a-repo");
        let mut config = CideConfig::default();

        let why = worktree_refusal(&root, &config).expect("refused outside a repository");
        assert!(why.contains("not a git repository"), "{why}");
        assert!(why.contains("git init"), "name a way out: {why}");
        assert!(why.contains("isolation"), "and the other one: {why}");

        // The escape hatch is real, and it is deliberately only reachable by hand-editing the
        // file — `OrchestrationPatch` cannot set `isolation`.
        config.agents.isolation = Isolation::Shared;
        assert_eq!(worktree_refusal(&root, &config), None);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The three answers stay three answers on the wire.**
    ///
    /// [`AgentIntegration`] is hand-mirrored in `ui/src/ipc/client.ts` because it is a local
    /// `serde::Serialize` rather than a ts-rs DTO, so nothing generates the TypeScript and
    /// nothing but this pins the JSON it is a mirror of. What is being protected is the
    /// distinguishability: "nothing to do", "merged, here is the commit" and "refused, here are
    /// the paths" are three different sentences and three different next actions, and any
    /// collapse of two of them — a boolean, an empty `paths` standing for success — reaches the
    /// user as a refusal that looks like a no-op.
    #[test]
    fn serialising_keeps_the_three_answers_apart() {
        let json = |value: &AgentIntegration| serde_json::to_value(value).expect("serialise");

        assert_eq!(
            json(&AgentIntegration::UpToDate),
            serde_json::json!({ "kind": "upToDate" })
        );
        assert_eq!(
            json(&AgentIntegration::Merged {
                commit: "0123456789abcdef".into(),
                files: 3,
            }),
            serde_json::json!({ "kind": "merged", "commit": "0123456789abcdef", "files": 3 })
        );
        assert_eq!(
            json(&AgentIntegration::Conflicts {
                paths: vec!["src/lib.rs".into(), "README.md".into()],
            }),
            // The paths are carried whole and in order. They are the entire value of a refusal
            // that changed nothing; a count here would be a dead end wearing a number.
            serde_json::json!({ "kind": "conflicts", "paths": ["src/lib.rs", "README.md"] })
        );
    }

    /// **An agent id is not a path, and the command inherits that refusal rather than repeating
    /// it.**
    ///
    /// `worktree::integrate` validates the name before it joins it onto `.cide/worktrees/` or
    /// builds a ref out of it, because an id comes from a committed config file that a model may
    /// have written. This asserts the command is in front of that check and not around it — a
    /// second copy of the whitelist here would be the copy that goes stale.
    ///
    /// It also asserts the *other* half: nothing is created. A refused integrate that left a
    /// directory behind would be `..`-relative mkdir with extra steps.
    #[test]
    fn an_agent_id_that_is_a_path_is_refused_and_nothing_is_created() {
        let root = temp("integrate-traversal");

        let why = integrate(&root, &AgentId("../../etc".into()), None)
            .expect_err("an id that is a path is refused");
        assert!(
            why.to_string().contains("../../etc"),
            "the refusal names what it refused: {why}"
        );
        assert!(!root.join(".cide").exists(), "a refusal created something");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A directory that is not a repository refuses with git's own sentence — **and not with the
    /// "no branch yet" one**, which is a claim about a role in a project that has one.
    #[test]
    fn integrating_outside_a_repository_says_that_and_not_something_about_branches() {
        let root = temp("integrate-not-a-repo");

        let why = integrate(&root, &AgentId("developer".into()), None)
            .expect_err("/tmp is not a git repository");
        let sentence = why.to_string();
        assert!(sentence.contains("git repository"), "{sentence}");
        assert!(
            !sentence.contains("no branch in this project yet"),
            "the wrong refusal: {sentence}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
