//! OpenSpec: reading a project's `openspec/`, and the five gestures that write it. (M28)
//!
//! Every one of these is a thin wrapper over `cide_spec`, which spawns the `openspec` CLI. Three
//! shapes here are decisions rather than habits.
//!
//! **Nothing is cached, and there is no store.** `cide_agents::load_project`'s posture, for its
//! stated reason: `openspec/` is committed, so a teammate's commit or a `git checkout` can
//! change it under a running app, and a cached board is a board that is wrong until something
//! happens to invalidate it. What `crate::spec_state` holds is a *coalescer*, not state.
//!
//! **Every mutation answers with the whole board, and emits.** `cmd::tasks`' rule: a call that
//! answered `{ok: true}` would be followed at once by a second asking what happened, and the
//! frame between them shows a screen that is visibly wrong. The emit is what keeps the *other*
//! windows right, and it goes through `spec_state::SpecBoards` rather than
//! `emit::spec_changed` directly — see that event's doc for why a second emit path would break
//! the property that lets it carry no revision.
//!
//! **Nothing here runs on the main thread.** Tauri polls a synchronous command on the GTK loop,
//! and every one of these spawns a Node process. Every handler is `async` and hands its work to
//! [`blocking`], `cmd::tasks`' reason exactly.
//!
//! **Two of the five never spawn the CLI.** `openspec/config.yaml` is read and written by
//! `cide_spec::config`, in-process, because `openspec config` manages *global* configuration and
//! will not edit the per-project file for you. See the block above [`spec_config_get`].
//!
//! **`archive` is the one gesture cide never takes on its own.** It rewrites files cide did not
//! open — merging deltas into `openspec/specs/`, the thing every later run reads as ground truth
//! — so it is reachable only from [`spec_accept`], which a person presses, after an integrate
//! that has to succeed first.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cide_core::CoreError;
use cide_ipc::{
    ChangeName, ProjectId, SpecBoard, SpecChange, SpecConfig, SpecConfigEdit, SpecRequirementSet,
    SpecSchema, SpecValidation, SpecWriteOutcome,
};
use cide_spec::{Openspec, SpecError};
use tauri::{AppHandle, Manager as _, State};

use crate::spec_state::SpecBoards;
use crate::tasks_state;
use crate::workspace_state::WorkspaceState;

type Result<T> = std::result::Result<T, CoreError>;

/// Run subprocess work on the blocking pool.
///
/// `cmd::tasks`' helper, and its doc's argument applies with more force here: these calls spawn a
/// Node process and wait up to sixty seconds for it. `#[tauri::command(async)]` would only move
/// that onto a runtime worker, where a blocking wait occupies it for the whole duration.
async fn blocking<T>(work: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T>
where
    T: Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| CoreError::Io(format!("the spec worker did not finish: {error}")))?
}

/// The project root, or the checkout of a live run.
///
/// `worktree` is a path the frontend may send for a change an agent is working on, and it is
/// **jailed**: it has to be inside this project's `.cide/worktrees/`. Without that, a panel could
/// name any directory on the machine and cide would run a subprocess in it.
fn working_dir(
    state: &State<'_, WorkspaceState>,
    project: ProjectId,
    worktree: Option<PathBuf>,
) -> Result<PathBuf> {
    let root = tasks_state::project_root(state, project)?;
    let Some(worktree) = worktree else {
        return Ok(root);
    };
    let canonical = worktree.canonicalize().unwrap_or_else(|_| worktree.clone());
    if !inside_worktrees(&root, &canonical) {
        return Err(CoreError::Io(format!(
            "{} is not one of this project's agent worktrees",
            worktree.display()
        )));
    }
    Ok(canonical)
}

/// Is `candidate` inside this project's `.cide/worktrees/`?
///
/// Pure, and separate from [`working_dir`], so the jail is a thing a test can state without a
/// Tauri `State` — a check that is only exercised through a command handler is a check nobody
/// asserts on.
///
/// # `..` is refused outright, and canonicalising is not enough on its own
///
/// `canonicalize` resolves symlinks and `..` — but only for a path that **exists**, and it
/// answers `Err` for one that does not. So a candidate naming a directory that is not there
/// falls back to the literal path, and `/repo/.cide/worktrees/../../../etc` is then a
/// component-wise prefix match on the jail while pointing at `/etc`. Refusing `..` before
/// looking at anything closes that without depending on what happens to be on disk.
///
/// The prefix test is component-wise and not textual, which is the other half:
/// `/repo/.cide/worktrees-evil` starts with the jail's *string* and is a different directory.
fn inside_worktrees(root: &std::path::Path, candidate: &std::path::Path) -> bool {
    use std::path::Component;
    if candidate
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return false;
    }
    let jail = root.join(".cide/worktrees");
    let jail = jail.canonicalize().unwrap_or(jail);
    let candidate = candidate
        .canonicalize()
        .unwrap_or_else(|_| candidate.to_path_buf());
    // The jail itself is not a worktree: there is nothing to run there.
    candidate.starts_with(&jail) && candidate != jail
}

/// Open the CLI for a directory, turning "no binary" into a `CoreError` the panel can print.
fn open(cwd: PathBuf) -> Result<Openspec> {
    Openspec::open(&cwd).map_err(|error| CoreError::Io(error.to_string()))
}

/// What this project's `openspec/` says right now.
///
/// Three shapes, not an array — `TaskBoard`'s argument: "this project does not use OpenSpec",
/// "the tool that reads it is missing" and "it is set up and nothing is in flight" are three
/// different screens with three different next actions.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_board(state: State<'_, WorkspaceState>, project: ProjectId) -> Result<SpecBoard> {
    let root = tasks_state::project_root(&state, project)?;
    blocking(move || Ok(crate::spec_state::read_at(&root))).await
}

/// One change in full: its deltas, its artifacts, its checklist and its verdict.
///
/// # And an archived one, which the CLI cannot answer at all
///
/// `openspec archive` moves the directory to `openspec/changes/archive/<stamp>-<name>/`. `show`
/// resolves `openspec/changes/<name>/proposal.md` and nothing else, so from the moment a change
/// is accepted this read fails — and the card of the task that did the work started saying *"it
/// could not be read; it may have been archived"*, which is a refusal at exactly the moment the
/// record is worth keeping.
///
/// So a failure falls through to `cide_spec::archived_change`, which reads the directory. The
/// answer is labelled [`cide_ipc::SpecOrigin::Archived`] and is deliberately smaller — deltas and
/// documents, no checklist and no verdict, because neither is recoverable from a directory
/// listing and neither means anything for work that is already merged.
///
/// The **CLI's** error is what surfaces when there is no archive either: it names the path it
/// looked at, which is the more useful sentence for the ordinary case of a mistyped or deleted
/// change.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_change(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    change: ChangeName,
    worktree: Option<PathBuf>,
) -> Result<SpecChange> {
    let cwd = working_dir(&state, project, worktree)?;
    blocking(move || match open(cwd.clone())?.change(&change) {
        Ok(live) => Ok(live),
        Err(error) => cide_spec::archived_change(&cwd, &change)
            .ok_or_else(|| CoreError::Io(error.to_string())),
    })
    .await
}

/// One artifact file's text — a proposal, a design, a checklist.
///
/// Fetched separately from [`spec_change`] because the panel draws those sections collapsed:
/// folding them into the change would read every document on every board refresh for prose
/// nobody has expanded. See `cide_spec::Openspec::artifact` for the jail and the size cap.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_artifact(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    path: PathBuf,
    worktree: Option<PathBuf>,
) -> Result<cide_ipc::SpecArtifactText> {
    let cwd = working_dir(&state, project, worktree)?;
    blocking(move || {
        let text = open(cwd)?
            .artifact(&path)
            .map_err(|error| CoreError::Io(error.to_string()))?;
        Ok(cide_ipc::SpecArtifactText {
            text: text.text,
            truncated: text.truncated,
        })
    })
    .await
}

/// Validate one change, or everything.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_validate(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    change: Option<ChangeName>,
    worktree: Option<PathBuf>,
) -> Result<SpecValidation> {
    let cwd = working_dir(&state, project, worktree)?;
    blocking(move || {
        open(cwd)?
            .validate(change.as_ref())
            .map_err(|error| CoreError::Io(error.to_string()))
    })
    .await
}

/// Set OpenSpec up in this project, and give it the project context in the same gesture.
///
/// Answers with the board so the panel can go from its pitch card to its tree in one round trip,
/// which is what makes the gesture feel like it did something.
///
/// # Why `context` is a parameter of *init* rather than a thing to set afterwards
///
/// `context:` is injected into every artifact-generation prompt: it is what an agent is told
/// about this project's stack and conventions before it writes a proposal, a design or a spec.
/// It is also prose somebody has to sit down and write, and the file `openspec init` leaves
/// behind states it only as a commented-out example — so the realistic outcome of "set it up now,
/// configure it later" is a project whose every agent prompt is context-free for ever.
///
/// The one moment a person is already thinking about what this project *is* is the moment they
/// are setting it up. So the wizard asks first and inits second, and the value rides along here
/// rather than going out as a second call the frontend could skip, fail or race.
///
/// `None` is the plain gesture with nothing asked — which is what the panel's own Set-up button
/// still sends, so the wizard is additive and that button is unchanged.
///
/// # Why the write happens after `init` and not before
///
/// `openspec init` creates `openspec/config.yaml`. Writing a context into a file that is about to
/// be created would either be overwritten or would make `init` refuse a directory that already
/// has one. `config::apply` is comment-preserving and splices into the file `init` wrote, so the
/// three commented example blocks — the only documentation these keys have — survive the wizard.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_init(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    boards: State<'_, Arc<SpecBoards>>,
    project: ProjectId,
    context: Option<String>,
) -> Result<SpecBoard> {
    let root = tasks_state::project_root(&state, project)?;
    let boards = Arc::clone(&boards);
    let board = blocking(move || {
        open(root.clone())?
            .init()
            .map_err(|error| CoreError::Io(error.to_string()))?;
        // Only when there is something to write. `tidy_block` turns a textarea the user tabbed
        // through and left alone into `None`, which writes no key at all — a `context: ""` in a
        // committed file is worse than no key, because the commented example that would have
        // told the next person what the key is for is then sitting under a key that exists.
        if let Some(context) = context.as_deref().map(tidy_block).filter(|c| !c.is_empty()) {
            cide_spec::config::apply(&root, &[cide_spec::config::Edit::Context(Some(context))])
                .map_err(|error| CoreError::Io(error.to_string()))?;
        }
        Ok(crate::spec_state::read_at(&root))
    })
    .await?;
    // The directory arriving is also a watcher event, so the other windows would learn about it
    // anyway — but a beat later, and through a path that is allowed to be slow. Marking here is
    // what makes them agree with this one immediately.
    boards.mark_changed(&app, project);
    Ok(board)
}

/// Propose a change: scaffold it, and give it a proposal.
///
/// **Not `openspec new change` alone**, which writes `README.md` and `.openspec.yaml` and no
/// proposal — and `openspec show` *refuses* a change with no proposal. A panel offering "new
/// change from this task" and calling only the scaffold would create a board row that opens onto
/// an error. See `cide_spec::Openspec::propose`.
///
/// # No UI reaches this any more, and that is the point
///
/// The compose dialog used to offer *Propose a new change* beside the change picker, and pressing
/// Create called this. What it produced was a change with a stub proposal, **no delta specs and
/// no task checklist** — which is to say a directory that `openspec validate` refuses and whose
/// board row reads `0/0` for ever. A scaffold is not a proposal; writing one *is* the work, and
/// the surface that does it is `/openspec-propose` running in a conversation that can read the
/// codebase. [`spec_propose_for_task`] is the gesture that replaced this one.
///
/// Kept as the primitive it is — it is the only correct way to *scaffold* a change, and
/// `cide_spec::Openspec::propose` is covered against the real CLI — but nothing should offer it
/// as a user-facing gesture again without a road that fills in the deltas.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_propose(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    boards: State<'_, Arc<SpecBoards>>,
    project: ProjectId,
    change: ChangeName,
    title: String,
    body: Option<String>,
) -> Result<SpecBoard> {
    let root = tasks_state::project_root(&state, project)?;
    let boards = Arc::clone(&boards);
    let board = blocking(move || {
        open(root.clone())?
            .propose(&change, &title, body.as_deref().unwrap_or_default())
            .map_err(|error| CoreError::Io(error.to_string()))?;
        Ok(crate::spec_state::read_at(&root))
    })
    .await?;
    boards.mark_changed(&app, project);
    Ok(board)
}

/// Replace one requirement block in one delta file.
///
/// The only place cide writes OpenSpec content. Its three outcomes are all *states the panel
/// draws* rather than errors — see [`SpecWriteOutcome`] — so a regression comes back with the
/// issues to show against the fields that caused them, and a conflict comes back naming the file
/// an agent is holding.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_requirement_set(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    boards: State<'_, Arc<SpecBoards>>,
    req: SpecRequirementSet,
    worktree: Option<PathBuf>,
) -> Result<SpecWriteOutcome> {
    let cwd = working_dir(&state, req.project, worktree)?;
    let project = req.project;
    let boards = Arc::clone(&boards);
    let outcome = blocking(move || {
        let os = open(cwd)?;
        cide_spec::write::set_requirement(
            &os,
            &req.change,
            &req.spec,
            req.operation,
            &req.requirement,
            &req.block,
        )
        .map_err(|error| CoreError::Io(error.to_string()))
    })
    .await?;
    if matches!(outcome, SpecWriteOutcome::Written { .. }) {
        boards.mark_changed(&app, project);
    }
    Ok(outcome)
}

/// Type one of OpenSpec's workflow commands into this project's Claude tab. (M28)
///
/// # Why cide types a command instead of explaining the workflow itself
///
/// `openspec init --tools claude` installs that workflow into the very session the pinned tab
/// runs. Those files are the workflow prose, written by the people who own the format and
/// versioned with the CLI. cide paraphrasing them would be a second wording that goes stale on
/// the next `npm i -g`.
///
/// # Why the command is checked against the directory, and not against a list in this file
///
/// Because a list here was wrong within a day, and then wrong again for a year. The first version
/// typed `/opsx:onboard`, which is in the CLI's *templates* and is **not** installed by its
/// default profile — so Claude answered `Unknown command: /opsx:onboard` and the button looked
/// broken in a way nothing in cide could explain. The second time it was the whole prefix: the
/// CLI moved from slash commands to skills, `/opsx:propose` became `/openspec-propose`, and every
/// button in the panel refused. `cide_spec::claude` is where both facts now come from, and its
/// header is the argument.
///
/// The refusal then names what *is* installed, because "that command is not here" is only useful
/// next to the ones that are.
///
/// # Why this refuses out loud
///
/// It shipped writing to the clipboard and swallowing every failure, and the button then did
/// nothing a user could see — the failure `claudeSend`'s header describes about the `mentionFile`
/// it replaced. Every arm below is an `Err` with a sentence, and the frontend deliberately does
/// not catch it, so `chrome/Failures.tsx` puts the reason on screen.
///
/// The typing goes through `agents::type_submitted_line`, the same path the orchestrator nudge
/// uses: the text now and the Enter a beat later, never one chunk, because the TUI's paste
/// detection is length-triggered and a line written whole lands in the composer with its CR
/// eaten.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_run_command(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    command: String,
    text: Option<String>,
) -> Result<()> {
    let root = tasks_state::project_root(&state, project)?;

    // The value reaches this parameter from the frontend, and it is about to be interpolated into
    // a line typed at a shell-adjacent TUI *and* joined onto a path. Kebab only, and the rule
    // lives beside the lookup so both uses are covered once.
    if !cide_spec::claude::valid_name(&command) {
        return Err(CoreError::Io(format!(
            "`{command}` is not an OpenSpec command name"
        )));
    }

    // Resolved here rather than taken from the board, and this is the load-bearing half: a
    // project set up — or migrated by `openspec update` — while cide was running is answered
    // correctly with no refresh, and a list that went stale in a panel can never put a name on a
    // terminal.
    let Some(invocation) = cide_spec::claude::line(&root, &command) else {
        let installed = cide_spec::claude::installed(&root);
        return Err(CoreError::Io(if installed.is_empty() {
            // Deliberately **not** `openspec update`, which is what this said for a year and is a
            // dead end: on a project whose tools are recorded and current it answers "all tools
            // up to date" and writes nothing at all.
            "this project has no OpenSpec commands for Claude Code. `openspec init --tools \
             claude` in the project root installs them — as skills under .claude/skills/ on a \
             current CLI, as slash commands under .claude/commands/opsx/ on an older one — and \
             cide runs whichever it finds."
                .to_string()
        } else {
            format!(
                "this project has no `{command}` command. What it has: {}. Which commands \
                 OpenSpec installs depends on the profile it was set up with, and cide does not \
                 choose it.",
                installed
                    .iter()
                    .map(|command| command.line.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }));
    };

    let session = state
        .with(|ws| {
            cide_core::workspace::project(ws, project).map(|project| project.primary_session)
        })
        .map_err(|_| {
            CoreError::Io("this project has no Claude session to send the command to".into())
        })?;

    let registry = app.try_state::<crate::state::SessionRegistry>();

    // ---------------------------------------------------------------------------------------
    // Is this conversation old enough to have missed the command?
    //
    // Claude Code reads a project's skills and slash commands **once, at startup**. Enabling
    // OpenSpec from the panel writes `.claude/skills/openspec-*/SKILL.md` into a project whose
    // console pane came up with the window — so the very next press of *Propose* typed
    // `/openspec-propose` into a `claude` that had never heard of it, and the conversation
    // answered `Unknown command: /openspec-propose`. Nothing in cide explained why, and the one
    // thing that fixes it — restarting that pane — is the one thing a user has no reason to try.
    //
    // Refused rather than sent, and the sentence names the gesture. `claude.restart` /
    // `claude.resume` are on the pane's own bar and in the palette, and resuming keeps the
    // transcript, so the cost of the fix is a few seconds.
    //
    // Setting up from inside cide no longer lands here: both of the panel's init doors restart
    // the console themselves afterwards (`ui/.../OpenSpecPanel/consoleReload.ts`), which moves
    // `started` past `installed_at`. This refusal is the backstop for every road that respawn
    // cannot cover — `openspec init` run in a terminal pane, a detached console whose restarter
    // lives in another window's realm, or a respawn that itself failed.
    if let Some(installed_at) = cide_spec::claude::installed_at(&root, &command)
        && let Some(started) = registry.as_ref().and_then(|reg| reg.started(session))
        && installed_at > started
    {
        return Err(CoreError::Io(format!(
            "`{invocation}` was installed into this project after this conversation started, so \
             Claude does not know it yet — Claude Code reads a project's skills once, when it \
             launches. Restart the Claude pane (its bar's Restart, or the palette's *Resume \
             Claude session*, which keeps the transcript) and press this again."
        )));
    }

    // **Flattened, and this is not cosmetic.** The line is *typed* into a PTY and terminated with
    // `\r`, so an embedded newline is another Enter: a two-line description would submit its
    // first line as a turn and feed the rest in as further turns. The failure looks exactly like
    // a model answering nonsense, which is why `opening_prompt` carries the same rule and why
    // this reuses its helper rather than restating it.
    let line = match text
        .as_deref()
        .map(crate::cmd::agents::one_line)
        .filter(|text| !text.is_empty())
    {
        Some(text) => format!("{invocation} {text}"),
        None => invocation,
    };
    let Some(bytes) = crate::agent_rpc::submit(&line) else {
        return Err(CoreError::Io(
            "this build cannot compose a line for the Claude CLI".into(),
        ));
    };

    let Some(pty) = registry.and_then(|sessions| sessions.get(session)) else {
        return Err(CoreError::Io(format!(
            "this project's Claude session is not running, so `{line}` has nowhere to go. Open \
             the project's Claude tab and try again."
        )));
    };

    tracing::info!(%project, %line, "typing an OpenSpec command into the product owner");
    crate::agents::type_submitted_line(&app, session, &pty, bytes);
    Ok(())
}

/// Open a change or a capability as a workspace tab, or activate the one it already has. (M28)
///
/// A singleton **per subject and per project**, `tab_open_extension`'s rule with the project half
/// put back: an extension page is the same README whichever project is open, while
/// `openspec/changes/add-dark-mode` is a different directory in every checkout that has one.
/// Opening one from two projects has to give two tabs or the second would silently show the
/// first's contents.
#[tauri::command(rename_all = "camelCase")]
pub async fn tab_open_spec(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    subject: cide_ipc::SpecSubject,
) -> Result<cide_ipc::TabId> {
    open_subject_tab(&state, project, subject)
}

/// [`tab_open_spec`]'s body, as a free function over the state — so a *trigger* can open the
/// same tab the panel row does. (M31)
///
/// Extracted rather than duplicated, and the reuse rule above is exactly why: a second
/// implementation that forgot to look for an existing tab would mint a duplicate every time a
/// proposal finished, and the two would look identical in the strip. `spec_reveal` is the caller
/// that made this necessary — see its header for what it reacts to.
pub(crate) fn open_subject_tab(
    state: &WorkspaceState,
    project: ProjectId,
    subject: cide_ipc::SpecSubject,
) -> Result<cide_ipc::TabId> {
    let wanted = subject;
    state.update(|ws| {
        let open = cide_core::workspace::project(ws, project)?;
        let existing = open
            .tabs
            .iter()
            .find(|tab| {
                matches!(&tab.kind, cide_ipc::TabKind::OpenSpec { subject } if *subject == wanted)
            })
            .map(|tab| tab.id);
        if let Some(id) = existing {
            cide_core::workspace::activate_tab(ws, project, id)?;
            return Ok(id);
        }
        cide_core::workspace::open_tab(
            ws,
            project,
            cide_ipc::TabKind::OpenSpec {
                subject: wanted.clone(),
            },
            cide_ipc::Pane {
                id: cide_ipc::PaneId::new(),
                kind: cide_ipc::PaneKind::Editor,
                role: cide_ipc::PaneRole::Auxiliary,
                // No process — the page renders over the tree, exactly as Settings and an
                // extension page do. See `tab_open_extension` for why the tree exists at all.
                session: None,
                conversation: None,
                conversation_since: None,
                continues: None,
                title: "openspec".into(),
                docker: None,
            },
        )
    })
}

/// Hand a task's work to a live Claude session. (M28)
///
/// # Why this is not a dispatch, and cannot start anything twice
///
/// Assigning a **role** is a dispatch gesture: `cide_agents::autodispatch`'s assignment edge sees
/// `Task::agent` change and starts a subagent through the queue. This writes
/// [`cide_ipc::TaskEdit::SetSession`] instead, which nothing downstream reads as a trigger — so
/// choosing an open conversation types the work into it and spawns nothing. The two targets are
/// different fields precisely so that "do not start it twice" is a property of the shape rather
/// than a check somebody has to remember; `Task::session`'s doc carries the argument.
///
/// `TaskStore::edit` clears `Task::agent` on the way through, so a task handed to a session stops
/// claiming a role at the same moment. One fact, one writer.
///
/// # The line is the same one a dispatched run is given
///
/// Built by [`crate::cmd::agents::opening_prompt`], not composed here: it is the sentence that
/// names the task, tells the reader to open it with `mcp__cide__cide_task_get` rather than
/// guessing, asks for a plan comment, and — since M28 — names the OpenSpec change when the task
/// has one. A second wording would drift from the one every subagent gets, and the difference
/// would be invisible until somebody compared two transcripts.
///
/// It is typed and not written, through the path the orchestrator nudge uses: the text now, the
/// Enter a beat later. The TUI's paste detection is length-triggered, and a long line written
/// whole lands in the composer with its CR eaten.
/// Start OpenSpec's propose workflow in a conversation, for a task that has no change. (M28)
///
/// # Why this exists, and what it replaced
///
/// The compose dialog used to offer *Propose a new change* next to the change picker, and Create
/// then called [`spec_propose`] — which scaffolds a directory with a stub proposal, **no delta
/// specs and no task checklist**. The result is a change `openspec validate` refuses and a board
/// row that reads `0/0` for ever: cide manufacturing a broken artefact out of a title.
///
/// Writing a proposal is *work*. It needs the codebase, the existing specs and a conversation —
/// which is exactly what `/openspec-propose` is, and cide already knows how to type it. So the
/// gesture moved from *before* the task exists to *after*: the card offers it, this sends it, and
/// the conversation writes a real proposal and links it back.
///
/// # The link is the agent's to make, and the line says how
///
/// cide cannot name the change in advance — the workflow chooses the folder name from what it
/// finds, and a name cide guessed from the title would be a link to a directory that never
/// appears. So the line names the task and the tool: `cide_task_update` takes a `change`, which
/// is what `TaskEdit::SetChange` is for, and the run has that tool because every dispatched or
/// bridged conversation gets the cide MCP server.
///
/// Answers the session the line went to, so the caller can reveal that pane — a command typed
/// into a conversation the user cannot see is indistinguishable from one that did nothing.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_propose_for_task(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    stores: State<'_, Arc<crate::tasks_state::TasksStores>>,
    project: ProjectId,
    task: cide_ipc::TaskId,
) -> Result<cide_ipc::SessionId> {
    let root = tasks_state::project_root(&state, project)?;
    let store = stores.ensure(project, &root);
    let Some(row) = store.get(&task) else {
        return Err(CoreError::Io(format!("no such task: {task}")));
    };
    if let Some(change) = row.change.as_ref() {
        return Err(CoreError::Io(format!(
            "this task already implements {change}. A task tracks one change — unlink it first if \
             the proposal was the wrong one."
        )));
    }

    let Some(invocation) = cide_spec::claude::line(&root, "propose") else {
        return Err(CoreError::Io(
            "this project has no OpenSpec propose command for Claude Code. `openspec init \
             --tools claude` in the project root installs one — as a skill under .claude/skills/ \
             on a current CLI, as a slash command under .claude/commands/opsx/ on an older one — \
             and cide runs whichever it finds."
                .into(),
        ));
    };

    /*
     * Which conversation, and the order is the whole of the decision.
     *
     * The task's own session first: if work on this task has already been handed somewhere, a
     * proposal for it belongs in that conversation and not in a second one that knows nothing
     * about it. The project's console pane otherwise, which is the conversation a user pressing
     * a button on a card is looking at.
     *
     * A dead session id on the task is not an error — the pane was closed, the record stays, see
     * `TaskView::session` — so it falls through rather than refusing.
     */
    let registry = app.try_state::<crate::state::SessionRegistry>();
    let primary = state
        .with(|ws| {
            cide_core::workspace::project(ws, project).map(|project| project.primary_session)
        })
        .map_err(|_| {
            CoreError::Io("this project has no Claude session to send the command to".into())
        })?;
    let session = row
        .session
        .filter(|id| {
            registry
                .as_ref()
                .and_then(|registry| registry.get(*id))
                .is_some()
        })
        .unwrap_or(primary);

    // `spec_run_command`'s rule, and its whole argument: Claude Code reads a project's skills
    // once, at startup, so a conversation older than the file answers `Unknown command`.
    if let Some(installed_at) = cide_spec::claude::installed_at(&root, "propose")
        && let Some(started) = registry
            .as_ref()
            .and_then(|registry| registry.started(session))
        && installed_at > started
    {
        return Err(CoreError::Io(format!(
            "`{invocation}` was installed into this project after this conversation started, so \
             Claude does not know it yet — Claude Code reads a project's skills once, when it \
             launches. Restart the Claude pane (its bar's Restart, or the palette's *Resume \
             Claude session*, which keeps the transcript) and press this again."
        )));
    }

    // One line, `opening_prompt`'s rule: this is *typed* into a PTY and terminated with `\r`, so
    // an embedded newline is another Enter and the task's body would submit itself in pieces.
    let title = crate::cmd::agents::one_line(&row.title);
    let body = crate::cmd::agents::one_line(&row.body);
    let mut line = format!("{invocation} {title}");
    if !body.is_empty() {
        line.push(' ');
        line.push_str(&body);
    }
    line.push_str(&format!(
        " (This proposal is for cide task {task}. When the change exists, call \
         mcp__cide__cide_task_update with task \"{task}\" and change set to the new change's \
         folder name under openspec/changes/, so the task and the change are linked, then comment \
         on the task saying what you proposed.)"
    ));

    let Some(bytes) = crate::agent_rpc::submit(&line) else {
        return Err(CoreError::Io(
            "this build cannot compose a line for the Claude CLI".into(),
        ));
    };
    let Some(pty) = registry.and_then(|registry| registry.get(session)) else {
        return Err(CoreError::Io(
            "this project's Claude session is not running, so the proposal has nowhere to go. \
             Open the project's Claude tab and try again."
                .into(),
        ));
    };

    tracing::info!(%project, %task, %session, "starting an OpenSpec proposal for a task");
    crate::agents::type_submitted_line(&app, session, &pty, bytes);
    Ok(session)
}

// ==========================================================================================
// Splitting a change into a task tree.
// ==========================================================================================

/// What cide types to ask a conversation to fan a change out into tasks. (M31)
///
/// # Why this is prose cide owns, where the neighbours type an upstream command
///
/// [`spec_run_command`] and [`spec_propose_for_task`] both resolve a line out of the project with
/// `cide_spec::claude`, because the *workflow* they start belongs to OpenSpec and a paraphrase
/// here would go stale on the next `npm i -g`. This one has no upstream counterpart: OpenSpec has
/// no opinion about `.cide/tasks.json`, about `subtaskOf`, or about who may dispatch a subagent.
/// The checklist is upstream's and is read as data; the decomposition is cide's.
///
/// # Every clause is load-bearing
///
/// * **`instructions apply --json`, never `tasks.md`.** Which file is the checklist is a schema
///   question — `cide_spec::Openspec::progress`'s doc carries the argument, and `openspec` answers
///   it. A model told to open `tasks.md` on a project whose schema names something else reads a
///   file that is not there and invents the steps.
/// * **"it outranks any summary of it including this one"** is
///   [`cide_agents::harness::SPEC_PREAMBLE`]'s move, verbatim and for its reason: a model handed
///   two overlapping instruction sets has to be told which one loses.
/// * **"no `change` field at all", with the consequence spelled out.** This is the invariant the
///   whole gesture turns on. `crate::spec_triggers::consider_one` moves a change's task
///   `Doing → Review` when its checklist finishes, and it finds that task by `Task::change`; on
///   more than one match it logs *several tasks name this change* and does nothing at all. So a
///   split that put the change on every subtask would silently switch that hop off for the life
///   of the change. A bare prohibition gets reasoned away by a model that thinks it is being
///   helpful; a stated consequence does not.
/// * **"Leave every task unassigned".** `cide_agents::autodispatch::trigger` fires on the
///   assignment *edge*, and a `cide_task_create` carrying an `assignee` is one — so a helpful
///   model that filled the field in would start N subagents on a plan nobody has read yet.
/// * **"do not do the work yourself".** `cide_agents::tools`' empty-roster answer was rewritten
///   for exactly this failure: *a model told "no roles" with no path is a model that gives up and
///   does the work itself without saying why*. Said here in advance, because by the time the
///   roster answers the model already has a checklist in front of it.
/// * **The ask is last and unconditional.** A model handed a checklist, a roster and
///   `cide_agent_dispatch` will dispatch. The first thing that happens after this button is a
///   question, and that is the whole difference between this and *Start work*.
///
/// One line, [`crate::cmd::agents::opening_prompt`]'s rule: it is typed into a PTY and terminated
/// with `\r`, so an embedded newline is another Enter and the back half would submit itself as a
/// turn of its own.
const SPLIT_PROMPT: &str = concat!(
    "Split the OpenSpec change {CHANGE} into cide tasks. ",
    "Its proposal, design and task checklist are in openspec/changes/{CHANGE}/ — run `{OPENSPEC} instructions apply --change {CHANGE} --json` from the repository root first and use the checklist it answers with as the source: that list is the work, in that order, it outranks any summary of it including this one, and it is how you learn which file the checklist actually is, so do not guess a filename. ",
    "Call mcp__cide__cide_task_create once for the main task, with a title naming the change and `change` set to \"{CHANGE}\" — exactly one task may carry that field, because cide moves one task per change to review when its checklist finishes and refuses to choose between two. ",
    "Then, for each coherent unit of that checklist, call mcp__cide__cide_task_create with a title, a body stating the work and naming the checklist steps it covers, `links` set to [{\"link\":\"subtaskOf\",\"target\":\"<the main task's id>\"}], and no `change` field at all; group the steps so each subtask is a piece somebody could finish and hand back on its own, and add a blockedBy link where one genuinely cannot start before another. ",
    "Leave every task unassigned: assigning a task starts a run on it immediately, and nothing is to start yet. ",
    "Then call mcp__cide__cide_agents_list. If this project defines roles, say which role you would give each subtask to and why. ",
    "If it defines none, do not do the work yourself — tell the user there is nobody to hand these to, and offer to write the roles they need as .cide/agents/<name>.md files, which take effect immediately. ",
    "Finally comment the plan on the main task with mcp__cide__cide_task_comment, then stop and ask the user whether to start; assign nothing and dispatch nothing until they answer, and when they say yes assign each subtask with mcp__cide__cide_task_assign, which is what starts its run.",
);

/// [`SPLIT_PROMPT`] with its two placeholders filled.
///
/// [`cide_agents::harness::spec_preamble`]'s shape and its fallback rule: `cli` is the absolute
/// path cide resolved, or `None` when it could not find one — in which case the bare word is used,
/// which is still a runnable line for a conversation whose own `PATH` has it and a far better
/// answer than declining to name the command. A free function over a path rather than a method on
/// anything, so the prose is reachable from a test with no `AppHandle`.
fn split_prompt(change: &str, cli: Option<&Path>) -> String {
    let openspec = match cli {
        Some(path) => path.display().to_string(),
        None => "openspec".to_string(),
    };
    SPLIT_PROMPT
        .replace("{CHANGE}", change)
        .replace("{OPENSPEC}", &openspec)
}

/// Ask this project's Claude session to turn a change's checklist into a main task and subtasks.
/// (M31)
///
/// # Why this is keyed on the change and not on a task
///
/// Because the task does not exist yet, and that is the point of the gesture. *Start work*
/// ([`crate::cmd::tasks::task_new`], through the page's own button) makes **one** task for a
/// change, which is the single-agent shape; this is the other one. cide writes nothing to the
/// tracker here — the conversation creates the main task, sets `change` on it, and hangs the
/// pieces off it as `subtaskOf` children, so the decomposition is one author's work rather than
/// half cide's and half the model's.
///
/// # Why the target is unconditionally the primary session
///
/// [`spec_propose_for_task`] prefers the task's own conversation and falls back to the primary.
/// This must not: `crate::agent_rpc::ProjectTools::names()` serves `cide_agents::tools::EVERY` —
/// the roster and the dispatch — **only** to the session that is `Project::primary_session`, and
/// every other pane gets `tools::ALL`, which has no `cide_agents_list` in it at all. A split typed
/// into any other conversation would reach the sentence telling it to look at the roster and find
/// no tool to look with.
///
/// # Why there is no skills-age guard
///
/// Its two neighbours both refuse when `cide_spec::claude::installed_at` is newer than the
/// session, because Claude Code reads a project's skills once at startup and would answer
/// `Unknown command`. This types cide's own prose and names no slash command, so that whole class
/// does not apply — noted here because the absence would otherwise read as an omission.
///
/// Answers the session the line went to, so the caller can reveal that pane: a line typed into a
/// conversation the user cannot see is indistinguishable from one that did nothing.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_split_work(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    stores: State<'_, Arc<crate::tasks_state::TasksStores>>,
    project: ProjectId,
    change: ChangeName,
) -> Result<cide_ipc::SessionId> {
    let root = tasks_state::project_root(&state, project)?;
    let store = stores.ensure(project, &root);

    // One task per change, and this is the gate rather than a hope. The page hides the button
    // once a change has a task, but a board the frontend read a moment ago is not evidence — and
    // the failure a second task causes is `spec_triggers::consider_one` declining to move
    // *either* of them for ever, which is a debug log nobody reads. Refused here, naming the task,
    // so it is a sentence on screen instead.
    if let Some(existing) = store
        .list()
        .into_iter()
        .find(|task| task.change.as_ref() == Some(&change))
    {
        return Err(CoreError::Io(format!(
            "{} already tracks {change}. A change gets one task carrying the link, and its pieces \
             hang off that task as subtasks — open {} and split from there, or unlink it first.",
            existing.id, existing.id
        )));
    }

    let session = state
        .with(|ws| {
            cide_core::workspace::project(ws, project).map(|project| project.primary_session)
        })
        .map_err(|_| {
            CoreError::Io("this project has no Claude session to send the split to".into())
        })?;

    // The resolved binary, for the same reason `harness::spec_preamble` carries one: `openspec` is
    // installed by `npm -g` into a directory a shell rc file puts on `PATH` and a desktop launcher
    // never does, so the bare word is a line that works in a terminal and not in this app. `open`
    // only runs the discovery ladder — it spawns nothing — and a failure here is not a refusal:
    // the bare word is still the right thing to print.
    let cli = Openspec::open(&root)
        .ok()
        .map(|os| os.binary().to_path_buf());
    let line = split_prompt(change.as_str(), cli.as_deref());

    let Some(bytes) = crate::agent_rpc::submit(&line) else {
        return Err(CoreError::Io(
            "this build cannot compose a line for the Claude CLI".into(),
        ));
    };
    let Some(pty) = app
        .try_state::<crate::state::SessionRegistry>()
        .and_then(|registry| registry.get(session))
    else {
        return Err(CoreError::Io(
            "this project's Claude tab is not running, so the split has nowhere to go. Open the \
             project's Claude tab and try again."
                .into(),
        ));
    };

    tracing::info!(%project, %change, %session, "asking the product owner to split a change");
    crate::agents::type_submitted_line(&app, session, &pty, bytes);
    Ok(session)
}

/// What this project types to hand a change's implementation to Claude, if anything.
///
/// `None` on two counts, and the caller treats them identically because the fallback is the same
/// prose either way:
///
/// * the project has no `apply-change` command — it was set up with a profile that does not
///   install one, or with no `--tools claude` at all;
/// * `started` says this conversation is older than the command's file. Claude Code reads a
///   project's skills once, at startup, so it would answer `Unknown command` — and unlike
///   `spec_run_command`, which exists *to* run that command and therefore refuses, a dispatch has
///   somewhere useful to go without it.
///
/// The name is cide's handle and not the line: the older surface spells this one `apply`, which
/// is `cide_spec::claude`'s business and not this module's.
fn apply_invocation(root: &Path, started: Option<std::time::SystemTime>) -> Option<String> {
    let invocation = cide_spec::claude::line(root, "apply-change")?;
    if let Some(installed_at) = cide_spec::claude::installed_at(root, "apply-change")
        && let Some(started) = started
        && installed_at > started
    {
        return None;
    }
    Some(invocation)
}

#[tauri::command(rename_all = "camelCase")]
pub async fn spec_dispatch_to_session(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    stores: State<'_, Arc<crate::tasks_state::TasksStores>>,
    project: ProjectId,
    task: cide_ipc::TaskId,
    session: cide_ipc::SessionId,
) -> Result<()> {
    use cide_ipc::{TaskAuthor, TaskEdit, TaskStatus};

    let root = tasks_state::project_root(&state, project)?;
    let store = stores.ensure(project, &root);

    // The session has to be one this project actually draws. The value comes from the frontend,
    // and typing a task into a conversation belonging to another project — or to a pane that
    // closed while the picker was open — is the failure this refuses rather than discovers.
    let known = state.with(|ws| {
        cide_core::workspace::project(ws, project).map(|project| {
            // Every Claude pane this project draws, the pinned tab's primary included. Walked
            // rather than looked up: a session id is a fact about a *pane*, and the workspace
            // tree is where panes live.
            project.primary_session == session
                || project.tabs.iter().any(|tab| {
                    tab.tree.panes.values().any(|pane| {
                        pane.kind == cide_ipc::PaneKind::Claude && pane.session == Some(session)
                    })
                })
        })
    });
    if !matches!(known, Ok(true)) {
        return Err(CoreError::Io(
            "that conversation is not one of this project's — it may have been closed while the \
             picker was open."
                .into(),
        ));
    }

    let Some(row) = store.get(&task) else {
        return Err(CoreError::Io(format!("no such task: {task}")));
    };

    // Recorded *before* the line is typed. If the write fails there is nothing in the
    // conversation to contradict, whereas a line typed into a session the tracker never learned
    // about is work nobody can find again.
    store
        .edit(
            &task,
            TaskEdit::SetSession {
                session: Some(session),
            },
            TaskAuthor::User,
        )
        .map_err(|error| CoreError::Io(error.to_string()))?;

    // `Todo → Doing`, and only that hop — `task_triggers::note_run_started`'s rule, for its
    // reason: a task already in `review` must not be yanked backwards by somebody opening it in a
    // second conversation.
    if row.status == TaskStatus::Todo {
        let _ = store.edit(
            &task,
            TaskEdit::SetStatus {
                status: TaskStatus::Doing,
            },
            TaskAuthor::Orchestrator,
        );
    }
    crate::tasks_state::broadcast(&app, project, &store);

    /*
     * The line, and which of two it is.
     *
     * A change's work has a *workflow* upstream — `/openspec-apply-change` reads the proposal,
     * the design and the checklist and knows the schema's own apply instruction — and typing
     * cide's paraphrase of that into a conversation was cide restating a thing it does not own.
     * So when the task names a change and the project has that command, the command **is** the
     * line and cide's task rules ride behind it as its argument. The rules still have to be
     * there: `openspec` knows nothing about `.cide/tasks.json`, the review hop, or who may
     * archive.
     *
     * `apply_invocation` answers `None` for a project that has no such command *and* for a
     * conversation too old to know it — see its doc. Both fall back to prose, which is what this
     * always sent and which still works. A dispatch must not fail because a skill file is newer
     * than a pane.
     */
    let refreshed = store.get(&task);
    // `ClaudeHarness` explicitly, not the dispatched role's: this line is *typed into a live
    // Claude pane*, whose own `cide-hook mcp` server is already attached and whose tools are
    // therefore spelled `mcp__cide__…` whatever harness the project's roles happen to name.
    let rules = crate::cmd::agents::opening_prompt(
        refreshed.as_ref(),
        None,
        Some(&cide_agents::harness::ClaudeHarness),
    );
    let started = app
        .try_state::<crate::state::SessionRegistry>()
        .and_then(|registry| registry.started(session));
    let line = match refreshed
        .as_ref()
        .and_then(|row| row.change.as_ref())
        .and_then(|change| {
            apply_invocation(&root, started).map(|invocation| (invocation, change.to_string()))
        }) {
        Some((invocation, change)) => format!("{invocation} {change}. {rules}"),
        None => rules,
    };
    let Some(bytes) = crate::agent_rpc::submit(&line) else {
        return Err(CoreError::Io(
            "this build cannot compose a line for the Claude CLI".into(),
        ));
    };
    let Some(pty) = app
        .try_state::<crate::state::SessionRegistry>()
        .and_then(|sessions| sessions.get(session))
    else {
        return Err(CoreError::Io(
            "that conversation is not running any more, so the task has nowhere to go. It is \
             still assigned to it — pick another, or reopen that tab."
                .into(),
        ));
    };
    tracing::info!(%project, %task, %session, "handing a task to a live conversation");
    crate::agents::type_submitted_line(&app, session, &pty, bytes);
    Ok(())
}

// ==========================================================================================
// Accepting a change: integrate, then archive, then done.
// ==========================================================================================

/// What accepting would do, without doing any of it.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_accept_preview(
    state: State<'_, WorkspaceState>,
    stores: State<'_, Arc<crate::tasks_state::TasksStores>>,
    project: ProjectId,
    task: cide_ipc::TaskId,
) -> Result<cide_ipc::SpecAcceptPlan> {
    let root = tasks_state::project_root(&state, project)?;
    let store = stores.ensure(project, &root);
    blocking(move || plan_accept(&root, &store, &task)).await
}

/// Merge the run's branch, archive the change, and close the task.
///
/// # The order is the reason this is one gesture and not two buttons
///
/// Integrate **first**. Archiving merges a change's deltas into `openspec/specs/`, which is what
/// every later run reads as ground truth — so archiving before the branch has landed would put
/// behaviour into the source of truth that the checked-out tree does not implement, and nothing
/// downstream could tell. A conflict therefore stops the whole gesture with nothing archived.
///
/// The plan is **re-run here** rather than trusted from the preview. A preview can be minutes
/// old, and in those minutes an agent can tick a box, a validator can start failing, or the user
/// can edit a delta — `start_child`'s expired-facts rule, applied to a gesture instead of a fork.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_accept(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    stores: State<'_, Arc<crate::tasks_state::TasksStores>>,
    boards: State<'_, Arc<SpecBoards>>,
    project: ProjectId,
    task: cide_ipc::TaskId,
) -> Result<cide_ipc::SpecAccepted> {
    use cide_ipc::{SpecAccepted, TaskAuthor, TaskEdit, TaskStatus};

    let root = tasks_state::project_root(&state, project)?;
    let store = stores.ensure(project, &root);
    let boards = Arc::clone(&boards);

    let store_for_work = Arc::clone(&store);
    let root_for_work = root.clone();
    let task_for_work = task.clone();
    let outcome = blocking(move || {
        let plan = plan_accept(&root_for_work, &store_for_work, &task_for_work)?;
        if !plan.refusals.is_empty() {
            return Ok(SpecAccepted::Refused { plan });
        }
        let Some(row) = store_for_work.get(&task_for_work) else {
            return Err(CoreError::Io(format!("no such task: {task_for_work}")));
        };
        let Some(change) = row.change.clone() else {
            return Err(CoreError::Io("this task names no OpenSpec change".into()));
        };

        // 1. The merge. A conflict returns here, with nothing archived.
        let (commit, files) = match row.agent.as_ref().filter(|_| plan.branch.is_some()) {
            Some(agent) => {
                match crate::cmd::agents::integrate_for(
                    &root_for_work,
                    agent,
                    Some(&task_for_work),
                )? {
                    crate::cmd::agents::AgentIntegration::Conflicts { paths } => {
                        return Ok(SpecAccepted::Conflicts { paths });
                    }
                    crate::cmd::agents::AgentIntegration::Merged { commit, files } => {
                        (Some(commit), files as u32)
                    }
                    crate::cmd::agents::AgentIntegration::UpToDate => (None, 0),
                }
            }
            // A `worktree: false` role commits into the checked-out tree, so there is nothing to
            // merge. Not a refusal — see `SpecAcceptPlan::branch`.
            None => (None, 0),
        };

        // 2. The archive, from the **project root**: the merge has just put the deltas there.
        open(root_for_work.clone())?
            .archive(&change)
            .map_err(|error| CoreError::Io(error.to_string()))?;

        // 3. And the task is done — authored `User`, because a person pressed a button, and
        //    since M27 `Task::history` records that claim where the card shows it.
        let _ = store_for_work.edit(
            &task_for_work,
            TaskEdit::SetStatus {
                status: TaskStatus::Done,
            },
            TaskAuthor::User,
        );
        Ok(SpecAccepted::Accepted {
            commit,
            files,
            change,
        })
    })
    .await?;

    if matches!(outcome, cide_ipc::SpecAccepted::Accepted { .. }) {
        crate::tasks_state::broadcast(&app, project, &store);
        boards.mark_changed(&app, project);
    }
    Ok(outcome)
}

/// What archiving this change would do, without a task in the picture.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_change_plan(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    change: ChangeName,
) -> Result<cide_ipc::SpecAcceptPlan> {
    let root = tasks_state::project_root(&state, project)?;
    blocking(move || plan_change(&root, &change)).await
}

/// Archive one change: merge its deltas into `openspec/specs/` and move it to the archive.
///
/// # This does not integrate a branch, and that is the difference from `spec_accept`
///
/// A change reached from the panel has no task, so there is no role, no worktree and no
/// `cide/<role>-<task>` to merge. If an agent *did* work this change through a task, archiving
/// from here would put its requirements into `specs/` while its code sat unmerged on a branch —
/// so the refusal table below stops that: a change whose task has a worktree is refused here and
/// pointed at the card, where Integrate & Archive does the two in the right order.
///
/// Everything else is `spec_accept`'s: the plan is re-run rather than trusted from a preview,
/// because a preview can be minutes old and an agent can tick a box inside them.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_change_archive(
    app: AppHandle,
    state: State<'_, WorkspaceState>,
    stores: State<'_, Arc<crate::tasks_state::TasksStores>>,
    boards: State<'_, Arc<SpecBoards>>,
    project: ProjectId,
    change: ChangeName,
) -> Result<cide_ipc::SpecAccepted> {
    use cide_ipc::SpecAccepted;

    let root = tasks_state::project_root(&state, project)?;
    let store = stores.ensure(project, &root);
    let boards = Arc::clone(&boards);

    let root_for_work = root.clone();
    let change_for_work = change.clone();
    let store_for_work = Arc::clone(&store);
    let outcome = blocking(move || {
        let mut plan = plan_change(&root_for_work, &change_for_work)?;

        // A task on this change whose role has a live worktree means there is code to merge, and
        // this path cannot merge it. Refuse and name where the gesture that can lives, rather
        // than writing requirements into `specs/` that the checked-out tree does not implement.
        let linked: Vec<cide_ipc::TaskRow> = store_for_work
            .list()
            .into_iter()
            .filter(|task| task.change.as_ref() == Some(&change_for_work))
            .collect();
        for task in &linked {
            let Some(agent) = task.agent.as_ref() else {
                continue;
            };
            let name = cide_agents::checkout_name(agent, Some(&task.id));
            let known = cide_git::worktree::list(&root_for_work).unwrap_or_default();
            if known.iter().any(|tree| tree.agent == name) {
                plan.refusals.push(format!(
                    "{} is working this change in .cide/worktrees/{name}, and archiving from \
                     here would not merge it — the requirements would land in openspec/specs/ \
                     while the code stayed on cide/{name}. Open task {} and use Integrate & \
                     Archive, which does both in that order.",
                    agent, task.id
                ));
            }
        }

        if !plan.refusals.is_empty() {
            return Ok(SpecAccepted::Refused { plan });
        }

        open(root_for_work.clone())?
            .archive(&change_for_work)
            .map_err(|error| CoreError::Io(error.to_string()))?;
        Ok(SpecAccepted::Accepted {
            commit: None,
            files: 0,
            change: change_for_work,
        })
    })
    .await?;

    if matches!(outcome, SpecAccepted::Accepted { .. }) {
        boards.mark_changed(&app, project);
    }
    Ok(outcome)
}

/// Everything accepting would do, and every reason it would not.
///
/// A free function over a path, `cmd::agents::integrate`'s shape, so the refusal table is
/// reachable from a test without an `AppHandle`.
fn plan_accept(
    root: &std::path::Path,
    store: &cide_tasks::TaskStore,
    task: &cide_ipc::TaskId,
) -> Result<cide_ipc::SpecAcceptPlan> {
    let Some(row) = store.get(task) else {
        return Err(CoreError::Io(format!("no such task: {task}")));
    };
    let Some(change) = row.change.clone() else {
        return Err(CoreError::Io(
            "this task names no OpenSpec change, so there is nothing to archive".into(),
        ));
    };

    let mut plan = plan_change(root, &change)?;

    // The branch half, which only a task has: worktrees are named per (role, task), so a change
    // with no task on it has nothing to merge and archives on its own. See `SpecAcceptPlan::branch`
    // for why `None` there is not a refusal.
    let checkout = row
        .agent
        .as_ref()
        .map(|agent| cide_agents::checkout_name(agent, Some(task)));
    plan.branch = checkout.as_ref().and_then(|name| {
        let known = cide_git::worktree::list(root).unwrap_or_default();
        known
            .iter()
            .any(|tree| tree.agent == *name)
            .then(|| format!("cide/{name}"))
    });
    Ok(plan)
}

/// Everything archiving one change would do, and every reason it would not — **without a task**.
///
/// # Why this exists separately
///
/// Because the gesture does. The whole accept path was built on the task card, on the reasoning
/// that the task is where the review conversation lives — and that left a change created straight
/// from the pinned session, by `/opsx:propose`, with no route to archive anywhere in the UI. That
/// is the more natural road, not the exceptional one: a change does not need a task to be
/// finished, and refusing to offer the last step of the lifecycle to somebody who did not use
/// cide's compose dialog is the panel declining to be a frontend to the format.
///
/// So the refusal table lives here, over a change alone, and the task-shaped entry point adds the
/// one thing a task knows about: which branch to merge first.
fn plan_change(
    root: &std::path::Path,
    change: &cide_ipc::ChangeName,
) -> Result<cide_ipc::SpecAcceptPlan> {
    use cide_ipc::{SpecAcceptPlan, SpecTouch};

    let os = open(root.to_path_buf())?;
    let detail = os
        .change(change)
        .map_err(|error| CoreError::Io(error.to_string()))?;
    let change = change.clone();

    let specs_touched: Vec<SpecTouch> = detail
        .deltas
        .iter()
        .map(|delta| SpecTouch {
            spec: delta.spec.clone(),
            operation: delta.operation,
            requirements: delta.requirements.len() as u32,
        })
        .collect();

    // Each refusal names the next action. A greyed control with no sentence is a dead control
    // in grey — `cide_core::commands`' rule, applied to a button.
    let mut refusals = Vec::new();
    if detail.progress.total > 0 && detail.progress.completed < detail.progress.total {
        refusals.push(format!(
            "{} of {} steps are still unticked in this change's task list. Archiving now would \
             put behaviour into openspec/specs/ that nobody has implemented.",
            detail.progress.total - detail.progress.completed,
            detail.progress.total
        ));
    }
    if !detail.validation.valid {
        for issue in detail.validation.issues.iter().filter(|i| i.blocking()) {
            refusals.push(format!("openspec validate: {}", issue.message));
        }
    }
    if specs_touched.is_empty() {
        refusals.push(
            "this change adds no requirements to any spec, so archiving it would move a folder \
             and change nothing. If that is really what you want, `openspec archive --skip-specs` \
             from a terminal says so deliberately."
                .to_string(),
        );
    }

    Ok(SpecAcceptPlan {
        change,
        // Filled in by `plan_accept` when a task names a role whose worktree exists.
        branch: None,
        completed_tasks: detail.progress.completed,
        total_tasks: detail.progress.total,
        specs_touched,
        refusals,
    })
}

/* ==============================================================================================
 * `openspec/config.yaml` — read and written as a form. (M28)
 *
 * The odd ones out in this module, and worth saying why: **these two do not spawn the CLI**.
 * `openspec` has a `config` command and it manages *global* configuration; the per-project file
 * is not something it will edit for you, and there is no `--json` road to it. So the reading and
 * the writing are `cide_spec::config`'s line-oriented scanner, in-process, and the only thing
 * here that forks a process is [`spec_schemas`].
 *
 * They still go through [`blocking`]. Reading is a `read` and a scan of a kilobyte, but writing
 * is an atomic rename of a file that may be open in an editor and held by an agent, and a
 * settings form that stuttered the GTK loop on Save is the failure `cmd::tasks`' rule exists to
 * prevent. Consistency here is also worth more than the microseconds: a handler in this module
 * that runs inline is one somebody will copy the next time.
 * ============================================================================================ */

/// What `openspec/config.yaml` states right now.
///
/// Never fails on a missing file: `config::read` answers with an empty config, because "there is
/// no config" and "there is a config that states nothing" are the same thing to every caller and
/// a project whose `openspec/` predates `config.yaml` is an ordinary project. [`SpecConfig::exists`]
/// carries the difference for the one caller that wants it — the form's heading.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_config_get(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
) -> Result<SpecConfig> {
    let root = tasks_state::project_root(&state, project)?;
    blocking(move || read_config(&root)).await
}

/// Apply a form's worth of edits, and answer with the config as it now reads.
///
/// # One call for the whole form, and the answer is the whole value
///
/// The module header's rule, and `config::Edit`'s own doc makes the same argument from the other
/// end: four setters would be four reads, four validations and four atomic renames of one
/// committed file for one press of Save. `config::apply` splices every edit into one buffer and
/// writes once — or, when every value already reads that way, does not write at all and does not
/// even move the mtime.
///
/// The answer is a re-read rather than the value the caller sent, which is not ceremony. A form
/// that trusted its own optimism would show `context` exactly as typed while the file held the
/// version the writer's normalisation produced, and the next reload would silently disagree with
/// the screen. Re-reading also means an edit the scanner declined to make — a key inside a
/// construct it does not understand, which it refuses to rewrite rather than guess at — comes
/// back as the value that is really there.
///
/// # Why nothing is emitted
///
/// `SpecBoard` does not carry the config, so `SpecBoards::mark_changed` would repaint every
/// window's panel with a board that had not changed. A second window with this form open does go
/// stale, which is the same gap `Settings → Agents` has for `.cide/agents/` and is admitted for
/// the same reason: the fix is an event carrying the config, and inventing one for a screen two
/// windows rarely hold at once would be a wire type and an emit path bought on speculation.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_config_set(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    edits: Vec<SpecConfigEdit>,
) -> Result<SpecConfig> {
    let root = tasks_state::project_root(&state, project)?;
    blocking(move || {
        let edits: Vec<cide_spec::config::Edit> = edits.into_iter().map(config_edit).collect();
        let wrote = cide_spec::config::apply(&root, &edits)
            .map_err(|error| CoreError::Io(error.to_string()))?;
        tracing::info!(%project, wrote, edits = edits.len(), "openspec config");
        read_config(&root)
    })
    .await
}

/// The workflow schemas this project can choose between.
///
/// # This one never fails, and that is deliberate
///
/// The config form has to work with no `openspec` on PATH at all: the file is read and written by
/// cide's own scanner, so a project can be configured on a machine where the CLI is missing, and
/// a `schema` select that refused to render because a subprocess could not be started would take
/// the whole screen down with it. Every failure — no binary, a CLI too old for `schemas --json`,
/// a document in a shape this build does not know — degrades to a single row for
/// `config::DEFAULT_SCHEMA`, logged at debug.
///
/// The caller then has to reconcile that with what the file states, because a select whose value
/// is not among its options renders as *some other option* and would rewrite a schema the user
/// chose on the first Save. `schemaOptions` in `sidebar/OpenSpecPanel/configModel.ts` is where
/// that happens, and `check:openspec-config` pins it.
#[tauri::command(rename_all = "camelCase")]
pub async fn spec_schemas(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
) -> Result<Vec<SpecSchema>> {
    let root = tasks_state::project_root(&state, project)?;
    blocking(move || Ok(read_schemas(&root))).await
}

/// `config::read` plus the three things a form needs and a reader does not.
fn read_config(root: &std::path::Path) -> Result<SpecConfig> {
    let config = cide_spec::config::read(root).map_err(|error| CoreError::Io(error.to_string()))?;
    let path = cide_spec::config::config_path(root);
    Ok(SpecConfig {
        schema: config.schema,
        default_schema: cide_spec::config::DEFAULT_SCHEMA.to_string(),
        context: config.context,
        rules: config
            .rules
            .into_iter()
            .map(|entry| cide_ipc::SpecArtifactRules {
                artifact: entry.artifact,
                rules: entry.rules,
            })
            .collect(),
        operations: config
            .operations
            .into_iter()
            .map(|entry| cide_ipc::SpecOperationGuidance {
                operation: entry.operation,
                guidance: entry.guidance,
            })
            .collect(),
        exists: path.is_file(),
        path,
    })
}

/// One wire edit as the domain edit it means.
///
/// A free function and not a `From` impl: both types are foreign to this crate, so the orphan
/// rule forbids one, and `cide-spec` may not depend on `ts_rs` to host it the other way round.
///
/// **The normalisation lives here rather than in the frontend**, because it is what keeps the
/// round-trip guarantee true and that guarantee is `cide_spec::config`'s, not a panel's. Two
/// rules, and both prevent a Save that rewrites a committed file for nothing:
///
///   * A list entry that is blank is dropped. A textarea row somebody opened and abandoned is not
///     a rule, and writing `- ` into a YAML sequence is a null item the CLI would then read.
///   * A block of prose is tidied the way the *reader* tidies it — see [`tidy_block`]. Without
///     that, a textarea whose value ends in a newline never compares equal to the value read back
///     out of the file, so every Save rewrites the file and every one shows up in `git status`.
fn config_edit(edit: SpecConfigEdit) -> cide_spec::config::Edit {
    use cide_spec::config::Edit;
    match edit {
        SpecConfigEdit::Schema { schema } => Edit::Schema(schema.trim().to_string()),
        SpecConfigEdit::Context { context } => Edit::Context(
            context
                .as_deref()
                .map(tidy_block)
                .filter(|text| !text.is_empty()),
        ),
        SpecConfigEdit::Rules { artifact, rules } => Edit::Rules {
            artifact: artifact.trim().to_string(),
            rules: tidy_list(rules),
        },
        SpecConfigEdit::OperationGuidance {
            operation,
            guidance,
        } => Edit::OperationGuidance {
            operation: operation.trim().to_string(),
            guidance: tidy_list(guidance),
        },
    }
}

/// A list of one-line strings, with the blanks gone and each one flattened.
///
/// Flattened because these become `- ` items in a YAML sequence, which is a *line*: a rule
/// somebody pasted a paragraph into would otherwise be written as several items, one of which is
/// the CLI's next parse error. The same rule `cmd::agents::one_line` states for a line typed at a
/// PTY, applied to a line spliced into a file.
fn tidy_list(items: Vec<String>) -> Vec<String> {
    items
        .into_iter()
        .map(|item| item.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|item| !item.is_empty())
        .collect()
}

/// A block of prose, normalised the way `config`'s block-scalar reader normalises it.
///
/// The reader maps a whitespace-only line to an empty one and drops trailing empties (clip
/// chomping, which is what `|` means and what `openspec init`'s own example writes). Sending a
/// value that has not been through the same normalisation means the writer compares "what the
/// user typed" against "what the file parses to" and finds them different every single time — so
/// a form that changed nothing would still rewrite the file, and a Save with no edit in it would
/// still turn up in a diff.
fn tidy_block(text: &str) -> String {
    let mut lines: Vec<&str> = text
        .lines()
        .map(|line| if line.trim().is_empty() { "" } else { line })
        .collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// The schemas the CLI lists, or the one row that is always true.
fn read_schemas(root: &std::path::Path) -> Vec<SpecSchema> {
    match schemas_from_cli(root) {
        Ok(schemas) if !schemas.is_empty() => schemas,
        Ok(_) => {
            // A CLI that ran and listed nothing. Not an error and not worth a screen: every
            // installation has `spec-driven`, so an empty list is a shape this build does not
            // understand rather than a project with no schemas.
            tracing::debug!("`openspec schemas --json` listed nothing; offering the default");
            vec![default_schema()]
        }
        Err(error) => {
            tracing::debug!(%error, "`openspec schemas --json` could not be read");
            vec![default_schema()]
        }
    }
}

/// `openspec schemas --json`, typed.
fn schemas_from_cli(root: &std::path::Path) -> std::result::Result<Vec<SpecSchema>, SpecError> {
    let os = Openspec::open(root)?;
    // `READ` and not `MUTATE`: this lists what is installed and touches nothing.
    let stdout = cide_spec::cli::run(
        os.binary(),
        os.cwd(),
        &["schemas", "--json"],
        cide_spec::cli::READ,
    )?;
    let rows: Vec<SchemaRow> = cide_spec::cli::parse(&stdout, "schemas --json")?;
    Ok(rows
        .into_iter()
        .filter(|row| !row.name.trim().is_empty())
        .map(|row| SpecSchema {
            name: row.name,
            description: row.description.filter(|text| !text.trim().is_empty()),
            artifacts: row.artifacts,
            source: row.source,
        })
        .collect())
}

/// The row `openspec schemas --json` prints, as of `@fission-ai/openspec` 1.8.0.
///
/// Lives here rather than in `cide_spec::model` only because that module is not this change's to
/// edit; everything else about it follows that module's posture, which is the deliberate opposite
/// of `cide-ipc`'s. Tolerant and `Deserialize`-only: every field carries a default, so a newer
/// CLI adding three fields still produces a list, and a field this build needs going missing is
/// caught by `cli::parse` with the command named.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SchemaRow {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    artifacts: Vec<String>,
    #[serde(default)]
    source: Option<String>,
}

/// The row every installation has, for when the CLI could not be asked.
fn default_schema() -> SpecSchema {
    SpecSchema {
        name: cide_spec::config::DEFAULT_SCHEMA.to_string(),
        description: None,
        artifacts: cide_spec::config::DEFAULT_ARTIFACTS
            .iter()
            .map(|artifact| (*artifact).to_string())
            .collect(),
        // Deliberately not `"package"`: this row is cide's guess, and labelling it with the
        // CLI's word for "shipped with the CLI" would be a claim about a document nobody read.
        source: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// The split prompt is one line, and that is structural rather than stylistic.
    ///
    /// It is typed into a PTY and terminated with `\r`, so an embedded newline is another Enter:
    /// the back half of the paragraph would submit itself as a turn of its own, which looks
    /// exactly like a model answering nonsense. `opening_prompt` holds the same invariant for the
    /// same reason.
    #[test]
    fn the_split_prompt_is_one_line() {
        let line = split_prompt("add-dark-mode", None);
        assert!(
            !line.contains('\n'),
            "an embedded newline is a second Enter"
        );
        assert!(!line.contains('\r'), "an embedded CR is a second Enter");
    }

    /// Every cide tool the prompt names is namespaced the way the model actually sees it.
    ///
    /// `cide_agents::harness`'s test, for its reason: the MCP server is mounted as `cide`, so a
    /// tool is `mcp__cide__cide_task_create` at the call site. Naming the bare `cide_task_create`
    /// is prose that is *almost* right — the model has to guess the prefix, and guessing wrong is
    /// a tool call that fails with nothing on screen explaining why.
    #[test]
    fn every_tool_the_split_prompt_names_is_namespaced() {
        let line = split_prompt("add-dark-mode", None);
        for (index, _) in line.match_indices("cide_task_") {
            assert!(
                line[..index].ends_with("mcp__cide__"),
                "a bare cide_task_* at byte {index} in the split prompt"
            );
        }
        for (index, _) in line.match_indices("cide_agents_list") {
            assert!(
                line[..index].ends_with("mcp__cide__"),
                "a bare cide_agents_list at byte {index} in the split prompt"
            );
        }
    }

    /// The three facts the whole gesture turns on are actually in the prose.
    ///
    /// Each of these is a silent failure the other way: without `subtaskOf` the run makes a flat
    /// list and the decomposition is lost; without `cide_agents_list` it never looks at the roster
    /// and cannot say who should take what; without the `change` prohibition
    /// `spec_triggers::consider_one` finds several tasks naming one change and stops moving any of
    /// them, logging to a file nobody reads.
    #[test]
    fn the_split_prompt_carries_the_three_rules() {
        let line = split_prompt("add-dark-mode", None);
        assert!(
            line.contains("subtaskOf"),
            "the edge kind is the decomposition"
        );
        assert!(
            line.contains("cide_agents_list"),
            "the roster is how it knows whom to name"
        );
        assert!(
            line.contains("no `change` field at all"),
            "one task per change is the invariant spec_triggers depends on"
        );
        assert!(
            line.contains("Leave every task unassigned"),
            "assigning is what starts a run, and nothing is to start yet"
        );
    }

    /// Both placeholders are filled, whichever answer the discovery ladder gave.
    #[test]
    fn split_prompt_leaves_no_placeholder_behind() {
        for cli in [
            None,
            Some(Path::new("/home/x/.nvm/versions/node/v22/bin/openspec")),
        ] {
            let line = split_prompt("add-dark-mode", cli);
            assert!(!line.contains("{CHANGE}"), "the change is unfilled");
            assert!(!line.contains("{OPENSPEC}"), "the binary is unfilled");
            assert!(line.contains("add-dark-mode"));
        }
        assert!(
            split_prompt("add-dark-mode", None).contains("`openspec instructions apply"),
            "a ladder that found nothing still names a line somebody's shell can run"
        );
    }

    /// A budget, not a measurement.
    ///
    /// `TRACKER_PREAMBLE`'s test and its argument: this is typed as one turn into a conversation
    /// that has work to do, and prose grows a sentence at a time with nobody noticing until it is
    /// a page. The number is loose on purpose — it fails when somebody doubles it, not when they
    /// add a clause.
    #[test]
    fn the_split_prompt_stays_within_its_budget() {
        let line = split_prompt("add-dark-mode", None);
        assert!(
            line.len() < 2_600,
            "the split prompt is {} bytes, which is a page rather than a brief",
            line.len()
        );
    }

    /// The two answers `apply_invocation` can give, and the reason the second one exists.
    ///
    /// A dispatch to a live conversation types OpenSpec's own apply workflow when the project has
    /// it — the workflow belongs upstream and cide's paraphrase of it was cide restating a thing
    /// it does not own. But Claude Code reads a project's skills **once, at startup**, so a
    /// conversation older than the file would answer `Unknown command`; there the answer is
    /// `None` and the caller falls back to the prose that always worked. A dispatch must not fail
    /// because a skill file is newer than a pane.
    #[test]
    fn a_dispatch_types_the_apply_workflow_unless_the_conversation_predates_it() {
        let root = std::env::temp_dir().join(format!(
            "cide-spec-apply-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);

        assert_eq!(
            apply_invocation(&root, None),
            None,
            "a project with no such command has nothing to type"
        );

        let entry = root.join(".claude/skills/openspec-apply-change/SKILL.md");
        std::fs::create_dir_all(entry.parent().expect("a parent")).expect("the directories");
        std::fs::write(&entry, "---\nname: openspec-apply-change\n---\n").expect("the file");

        assert_eq!(
            apply_invocation(&root, None).as_deref(),
            Some("/openspec-apply-change"),
            "no session time is no claim about staleness — a run with no pane is not stale"
        );

        let installed_at =
            cide_spec::claude::installed_at(&root, "apply-change").expect("the file has an mtime");
        assert_eq!(
            apply_invocation(
                &root,
                Some(installed_at - std::time::Duration::from_secs(60))
            ),
            None,
            "a conversation older than the command cannot know it, so cide sends prose instead"
        );
        assert_eq!(
            apply_invocation(
                &root,
                Some(installed_at + std::time::Duration::from_secs(60))
            )
            .as_deref(),
            Some("/openspec-apply-change"),
            "and one started after it can"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The config form's normalisation, stated in the same cases `check-openspec-config.mjs`
    /// states in TypeScript — the two mirror each other, so a divergence shows as one side
    /// passing.
    ///
    /// Every case here is a way a *textarea* differs from what `config`'s reader produces. Get
    /// one wrong and nothing throws: the value simply never compares equal to the file's, so
    /// every Save rewrites a committed file having changed nothing anybody typed.
    #[test]
    fn a_textarea_is_normalised_into_what_the_reader_would_read_back() {
        // A textarea ends in a newline; a clip-chomped block scalar does not.
        assert_eq!(tidy_block("a\n"), "a");
        assert_eq!(tidy_block("a\n\n\n"), "a");
        // A whitespace-only line reads back as an empty one.
        assert_eq!(tidy_block("a\n   \nb"), "a\n\nb");
        // Indentation inside the block is content, and must survive.
        assert_eq!(tidy_block("  a\n    b"), "  a\n    b");
        // A box with nothing but whitespace in it is no context at all.
        assert_eq!(tidy_block("   \n  "), "");
        assert_eq!(tidy_block(""), "");
    }

    #[test]
    fn a_rule_is_one_line_whatever_was_pasted_into_the_box() {
        // A rule becomes a `- ` item in a YAML sequence, which is a *line*: a pasted paragraph
        // written as several items is the CLI's next parse error.
        assert_eq!(
            tidy_list(vec!["two\nlines".into()]),
            vec!["two lines".to_string()]
        );
        assert_eq!(
            tidy_list(vec!["  keep   it  short  ".into()]),
            vec!["keep it short".to_string()]
        );
        // A row somebody opened with Add and abandoned is not a rule.
        assert_eq!(
            tidy_list(vec!["a".into(), "   ".into(), "b".into()]),
            vec!["a".to_string(), "b".to_string()]
        );
        assert!(tidy_list(vec![String::new()]).is_empty());
    }

    /// An emptied context box removes the key rather than writing `context: ""`.
    ///
    /// An empty key with the commented example sitting under it is worse than no key: the
    /// example is the only documentation the key has, and a `context: ""` above it reads as
    /// "this is configured, and it is configured to nothing".
    #[test]
    fn an_emptied_box_removes_the_key_rather_than_writing_an_empty_one() {
        use cide_spec::config::Edit;
        assert_eq!(
            config_edit(SpecConfigEdit::Context {
                context: Some("  \n ".into())
            }),
            Edit::Context(None)
        );
        assert_eq!(
            config_edit(SpecConfigEdit::Context { context: None }),
            Edit::Context(None)
        );
        assert_eq!(
            config_edit(SpecConfigEdit::Context {
                context: Some("Tech stack: Rust.\n".into())
            }),
            Edit::Context(Some("Tech stack: Rust.".into()))
        );
    }

    /// The row cide offers when the CLI could not be asked is the one every installation has.
    #[test]
    fn the_fallback_schema_row_is_the_cli_default_and_its_artifacts() {
        let row = default_schema();
        assert_eq!(row.name, cide_spec::config::DEFAULT_SCHEMA);
        assert_eq!(
            row.artifacts,
            cide_spec::config::DEFAULT_ARTIFACTS
                .iter()
                .map(|a| (*a).to_string())
                .collect::<Vec<_>>()
        );
        // Not labelled `package`: this row is cide's guess, and borrowing the CLI's word for
        // "shipped with the CLI" would be a claim about a document nobody read.
        assert_eq!(row.source, None);
    }

    /// `openspec schemas --json`, as version 1.8.0 really prints it.
    ///
    /// The artifacts are the half that matters: `DEFAULT_ARTIFACTS` is documented as a default
    /// and **not** a closed set, so a rules editor that offered four hard-coded rows would offer
    /// the wrong ones on any custom schema with nothing on screen to say so.
    #[test]
    fn a_schemas_document_is_read_into_names_and_the_artifacts_each_declares() {
        let stdout = br#"[{"name":"spec-driven",
            "description":"Default OpenSpec workflow - proposal -> specs -> design -> tasks",
            "artifacts":["proposal","specs","design","tasks"],"source":"package"},
            {"name":"lean","artifacts":["proposal","tasks"]}]"#;
        let rows: Vec<SchemaRow> =
            cide_spec::cli::parse(stdout, "schemas --json").expect("a real document parses");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "spec-driven");
        assert_eq!(rows[0].artifacts, ["proposal", "specs", "design", "tasks"]);
        // Tolerant, `cide_spec::model`'s posture: a row with no description and no source is a
        // row, not a parse failure, because a newer CLI dropping a field must still list schemas.
        assert_eq!(rows[1].description, None);
        assert_eq!(rows[1].source, None);
    }

    /// The typed line is one line, whatever the user wrote in the box.
    ///
    /// It is typed into a PTY and ended with `\r`, so an embedded newline is another Enter: a
    /// description written across two lines would submit its first line as a turn and feed the
    /// rest in as further turns. The failure looks exactly like a model answering nonsense, which
    /// is why this is asserted rather than left to a reader to notice — `opening_prompt` carries
    /// the identical rule one module over, and both use the same helper.
    #[test]
    fn a_command_line_is_one_line_whatever_was_typed_into_it() {
        let flat = crate::cmd::agents::one_line;
        assert_eq!(
            flat("add dark mode\nand remember it\tacross restarts"),
            "add dark mode and remember it across restarts"
        );
        assert_eq!(flat("   "), "");
        assert_eq!(flat(""), "");
        assert!(!flat("a\r\nb").contains('\r'));
        // A carriage return alone is the terminator itself, and would submit mid-sentence.
        assert!(!flat("a\rb").contains('\r'));
    }

    /// The jail is the whole of what `worktree` is allowed to mean.
    ///
    /// Without it a panel could name any directory on the machine and cide would spawn a
    /// subprocess in it — and this parameter's value comes from the frontend, which ADR 0002
    /// describes as a mirror rather than a source of truth.
    #[test]
    fn a_worktree_outside_the_project_is_refused() {
        let root = Path::new("/repo");

        assert!(inside_worktrees(
            root,
            Path::new("/repo/.cide/worktrees/dev")
        ));
        assert!(inside_worktrees(
            root,
            Path::new("/repo/.cide/worktrees/dev-t-17/openspec")
        ));

        // The ordinary misses.
        assert!(!inside_worktrees(root, Path::new("/repo/src")));
        assert!(!inside_worktrees(root, Path::new("/etc")));
        assert!(!inside_worktrees(root, Path::new("/repo/.cide/agents")));
        assert!(!inside_worktrees(
            root,
            Path::new("/elsewhere/.cide/worktrees/dev")
        ));

        // The jail itself is not a worktree — there is nothing to run there.
        assert!(!inside_worktrees(root, Path::new("/repo/.cide/worktrees")));

        // And the two that a string prefix would let through.
        assert!(!inside_worktrees(
            root,
            Path::new("/repo/.cide/worktrees-evil/dev")
        ));
        assert!(!inside_worktrees(
            root,
            Path::new("/repo/.cide/worktrees/../../../etc")
        ));
    }
}
