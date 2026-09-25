//! Opening a Claude tab that cide asked for, rather than a person. (M79)
//!
//! Two features need the same thing: a fresh `claude`, in its own closable tab, already carrying
//! a prompt. A finished subagent turn opens one to *review* the work with nothing else in its
//! context ([`crate::agent_rpc`]), and a project that has gone quiet with work still open gets
//! one to plan the next round ([`crate::spinner`]). Both are decided in Rust, on a background
//! thread, with no webview gesture anywhere in the story — so this is where the tab is built.
//!
//! # Why a pane built in Rust is right here, when `agent_open_pane` was wrong
//!
//! `cmd::agents` carries a long note on a deleted command that built a pane and answered with
//! its id, and the failure it caused: a pane created by a command arrives over
//! `cide://workspace-changed`, so it never passes through `rememberSpawnPlan`/`takeSpawnPlan`,
//! so `PaneHost.mirrored` is never set — and `closePane` reads exactly that flag to decide
//! whether the pane owns the child it holds. **Closing an agent's pane killed the agent's
//! `claude` mid-turn.**
//!
//! That note is about a pane *adopting somebody else's* session. This pane's child was forked
//! **for it**, seconds earlier, by this function. It owns it. So `mirrored` staying false is not
//! a bug being re-introduced by a second door — it is the correct value, and it is what makes
//! closing the pane end the child, which is what anybody pressing Ctrl+W on a review tab means.
//! The two cases differ in how the pane came to exist, which is what the note says the spawn
//! plan records; here there is nothing to record because there was never another owner.
//!
//! # Session first, tab second
//!
//! `SplitIntent::Adopt`'s rule, and the whole of the M50 compose fix: a plan parked between a
//! mutation committing and a pane mounting can be lost, because the broadcast can render the
//! pane before the command that created it has returned. A compose pane that lost its plan
//! opened **a login shell**. Here the id goes straight into `Pane::session`, which is durable,
//! so nothing has to survive anything — and `TerminalPane`'s adoption branch (`domainSession &&
//! !spawns && !host.sessionId`, guarded by `sessionIsHeld`) finds a session the registry already
//! holds and attaches to it instead of forking a second child.
//!
//! # The tab is ephemeral, and that is a decision about leaking
//!
//! `TabKind::ClaudeFull::ephemeral`'s doc carries it: closing a tab normally parks its sessions
//! for Ctrl+Shift+T, which at one tab per finished run is a `claude` leaked per run. Every tab
//! this module opens is marked, and `cmd::project::tab_close` ends a marked tab's children.

use cide_core::CoreError;
use cide_ipc::{Geometry, Pane, PaneId, PaneKind, PaneRole, ProjectId, SessionId, TabId, TabKind};
use tauri::Manager;

use crate::workspace_state::WorkspaceState;

/// Which permission mode the new child runs under.
///
/// **Both tabs are unattended**, which is the fact that shapes this. These tabs are opened by
/// cide, not by a person reaching for one, and they are opened *because* nobody is watching: one
/// when a subagent finishes while the product owner is elsewhere, one when the whole project has
/// been quiet for a quarter of an hour. A prompt either of them raises is a prompt nobody
/// answers, and the child sits there holding a tab — the same silent failure
/// `AgentsConfig::permission_mode` was written for, measured on dispatched runs and no less
/// true here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TabMode {
    /// The project's own `AgentsConfig::unattended`, spelled the way `harness::claude` spells it
    /// for a dispatched run: one stance for every unattended child cide starts, set in one place.
    ///
    /// # The difference from a dispatched run, stated
    ///
    /// A reviewer stands in the **worktree it is reviewing** (see `open_with_prompt`'s `cwd`), so
    /// it has a dispatched run's containment and for the same reason. What it does not have is a
    /// checkout of its *own*: it is standing in the one the run it is reviewing used, which is
    /// why `agent_rpc::review_prompt` tells it in as many words not to fix anything itself and
    /// to hand the task back to the role that has the context. That last part is a brief rather
    /// than a mechanism, which is why this follows the project's setting rather than forcing the
    /// stance: a project whose `permissionMode` is `manual` gets a reviewer that asks, and a tab
    /// that asks is at least visible. The spinner's planner stands in the project root and has
    /// no containment but its brief, on the same argument.
    ///
    /// # Why there is no plan mode any more (M88)
    ///
    /// The spinner's tab was `--permission-mode plan` from M79, so it would survey before it
    /// acted. Plan mode asks before **every** MCP call — it is the CLI's `default` with edits
    /// withheld — and the survey is nothing but MCP calls, so an unattended planner parked on a
    /// `cide_task_get` approval; reported as exactly that. And it ends at an `ExitPlanMode`
    /// approval no hook can answer (that tool's own `checkPermissions` returns `ask`), so cide
    /// read the prompt off the rendered grid and typed the answer, `cide_claude::plan`. Two
    /// workarounds, deleted together, for a mode whose one job here the prompt already states.
    pub unattended: cide_agents::Unattended,
    /// Open behind the tab the user is on, rather than raising it.
    ///
    /// A reviewer opens *because* a subagent finished while the user was reading something else,
    /// so raising it takes the view away mid-sentence; the spinner's tab raises, because it fires
    /// only after the project has been idle, and is about to plan the next round — the thing
    /// somebody coming back wants in front of them. See the `behind` block in the body.
    pub behind: bool,
}

/// Which CLI a tab cide opens runs. (M104)
///
/// Until M104 this was always Settings → Harness, read inside [`open`]. `cide_session_open` lets
/// the orchestrator choose per tab, and adds the one CLI that is not a console: opencode, whose
/// TUI runs as a program in a `Shell`-kind pane — `cmd::pane::pane_for` makes the same call for a
/// re-opened opencode conversation, and `cide_ipc::ConsoleHarness`' doc is why opencode is not
/// made a console to get here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabHarness {
    Console(cide_ipc::ConsoleHarness),
    Opencode,
}

/// Everything [`open`] needs beyond the project. (M104)
///
/// A struct because the list grew past what a reader can match positionally: two of M79's
/// parameters were already `Option`s of the same shape, and M104 adds four more.
pub(crate) struct Open<'a> {
    pub title: &'a str,
    /// `claude --name`: what the CLI shows in its prompt box and in `/resume`. `None` leaves the
    /// session unnamed, which is what a reviewer wants — its tab title is a task, not a name.
    pub session_name: Option<&'a str>,
    pub prompt: &'a str,
    pub mode: TabMode,
    /// Where the child stands. `None` is the project root; `Some` is a worktree — see the `cwd`
    /// note in the body.
    pub cwd: Option<std::path::PathBuf>,
    /// `None` is Settings → Harness, which is what every M79 caller wants.
    pub harness: Option<TabHarness>,
    /// Passed as the CLI's `--model`. `None` is the CLI's own default.
    pub model: Option<&'a str>,
    /// Which paragraph the child is told it is. [`Voice::Acting`] for the reviewer and the
    /// planner; [`Voice::Worker`] for a `cide_session_open` tab.
    pub voice: crate::cmd::session::Voice,
    /// Recorded on the pane — see `cide_ipc::Pane::origin`.
    pub origin: Option<cide_ipc::PaneOrigin>,
}

/// Spawn a `claude`, open a tab holding it, and type `prompt` into it.
///
/// The M79 entry point, kept for its two callers: Settings → Harness, the product owner's voice.
/// [`open`] is the whole of it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn open_with_prompt(
    app: &tauri::AppHandle,
    project: ProjectId,
    title: &str,
    session_name: Option<&str>,
    prompt: &str,
    mode: TabMode,
    cwd: Option<std::path::PathBuf>,
) -> Result<(SessionId, TabId), CoreError> {
    open(
        app,
        project,
        Open {
            title,
            session_name,
            prompt,
            mode,
            cwd,
            harness: None,
            model: None,
            voice: crate::cmd::session::Voice::Acting,
            origin: None,
        },
    )
}

/// Spawn the tab's CLI, open a tab holding it, and give it `prompt`.
///
/// Answers the new session and tab once the child exists and the tree has been mutated. The
/// prompt is delivered asynchronously by [`crate::agents::type_submitted_line`], which returns
/// immediately and finishes on its own thread — so a caller is not blocked for the several
/// seconds a TUI takes to boot.
///
/// **Callable only from a thread that is neither the GTK loop nor a Tauri runtime worker**: it
/// enters the runtime with `block_on` to reach the spawn, which is `async` because forking a
/// process is not work for the webview's thread. `crate::agent_rpc`'s header states the rule.
pub(crate) fn open(
    app: &tauri::AppHandle,
    project: ProjectId,
    open: Open<'_>,
) -> Result<(SessionId, TabId), CoreError> {
    let Open {
        title,
        session_name,
        prompt,
        mode,
        cwd,
        harness: chosen,
        model,
        voice,
        origin,
    } = open;
    let state = app
        .try_state::<WorkspaceState>()
        .ok_or_else(|| CoreError::Io("the workspace is going away".into()))?;

    // The first root, which is what a Claude pane's `cwd` is everywhere else in this app. A
    // project with none cannot host a `claude` at all, and saying so here is better than
    // forking one into whatever directory cide happens to be in.
    // Settings → Harness decides which CLI a tab cide opens runs (M93), exactly as it decides a
    // fresh console: these tabs *are* consoles, doing the product owner's job.
    let (root, name, setting, llm, codex_policy) = state.with(|ws| {
        let project = cide_core::workspace::project(ws, project)?;
        let root = project
            .roots
            .first()
            .map(|root| root.path.clone())
            .ok_or(CoreError::NoRoots)?;
        Ok::<_, CoreError>((
            root,
            project.name.clone(),
            ws.settings.console_harness,
            ws.settings.llm.clone(),
            ws.settings.codex.cli.inject.permissions,
        ))
    })?;
    let tab_harness = chosen.unwrap_or(TabHarness::Console(setting));
    let codex = tab_harness == TabHarness::Console(cide_ipc::ConsoleHarness::Codex);
    let opencode = tab_harness == TabHarness::Opencode;
    // The console identity where there is one; opencode borrows claude's only for the argv arm
    // below that it never reaches.
    let harness = match tab_harness {
        TabHarness::Console(console) => console,
        TabHarness::Opencode => cide_ipc::ConsoleHarness::Claude,
    };
    let model = model.map(str::trim).filter(|m| !m.is_empty());

    // One flag, spelled once. `Ask` passes nothing, which is the CLI's own default.
    let mut args: Vec<String> = Vec::new();
    if codex {
        // Codex's vocabulary for the same three answers: the sandbox-and-approval pair a run of
        // that mode gets (`harness::codex::permission_policy`), and nothing for `Ask`, where the
        // user's own `config.toml` decides and codex asks in the pane. No `--name`: codex has
        // none, and names a thread by itself.
        //
        // And nothing when Settings says a wrapper decides (M108, `CodexInjections::permissions`).
        let policy = match mode.unattended {
            _ if !codex_policy => None,
            cide_agents::Unattended::Ask => None,
            unattended => cide_agents::harness::codex::permission_policy(None, unattended).ok(),
        };
        args.extend(policy.into_iter().flatten().map(|t| t.to_string()));
    } else {
        let permission = match mode.unattended {
            cide_agents::Unattended::Auto => Some("auto"),
            cide_agents::Unattended::Bypass => Some(cide_agents::defs::BYPASS_PERMISSIONS),
            cide_agents::Unattended::Ask => None,
        };
        if let Some(permission) = permission {
            args.push("--permission-mode".into());
            args.push(permission.into());
        }
        if let Some(name) = session_name {
            args.push("--name".into());
            args.push(name.into());
        }
    }
    // Both consoles spell it the same. Opencode's goes through `tab_launch` below.
    if !opencode && let Some(model) = model {
        args.push("--model".into());
        args.push(model.into());
    }

    // `Geometry::default()` is 80×24, byte-identical to the webview's own `FALLBACK`, and it
    // self-corrects: `session_attach` resizes to the pane's measured geometry *before* it
    // attaches a sink, and `ensureAttached` runs a `syncSize` after the snapshot. Nothing here
    // can know the size, because the pane it is for does not exist yet.
    // **A reviewer stands in the worktree it is reviewing.** (M79)
    //
    // Two things follow from it, and the second is the reported bug. The obvious one: the diff
    // and the tests it is told to run are *there*, so it starts in the branch under review
    // rather than one `cd` away from it.
    //
    // The one that was reported: `claude` files a transcript under **the directory it started
    // in**, so every reviewer standing in the project root landed in that project's own Claude
    // history — sixty-one of them in a day on a real project, drowning the conversations the
    // user actually goes there to find. Worse where it was noticed: that project runs its
    // subagents on `opencode`, so cide's reviewers were the *only* claude sessions in it, and
    // "the project's Claude history" had become a list of cide's own bookkeeping. Under the
    // worktree they file beside the work they are about, which is where somebody looking for
    // them would think to look.
    //
    // `cide_agent_integrate` is unaffected and that is what makes this safe: it resolves the
    // project root from the workspace and merges into the branch *that* checkout has out, not
    // into whatever the caller is standing in (`agent_rpc::RegistrySink::integrate`).
    //
    // Falls back to the root, which is right for a run dispatched with no task — it worked in
    // the root itself (M40) — and for a worktree that has since been removed.
    let cwd = cwd.filter(|path| path.is_dir()).unwrap_or(root.clone());

    // **The pane has to remember that directory, or the conversation is lost at the first
    // restart.** Reported as a Resume that worked half the time and otherwise opened a blank
    // `claude`: the half that failed were exactly the reviewers standing in a worktree.
    // `lifecycle::entry_for` looks for a pane's transcript under the project root unless
    // `Pane::continues` names another directory, so a reviewer's — filed under the worktree,
    // by the paragraph above — answered `Fresh`, and the splash's button started a new session
    // in the root. `continues` is M42's record of exactly this fact for a run's pane, and every
    // reader already honours it: the restore plan, the cwd `App.tsx` hands the pane, and
    // `session_spawn`, which respawns `claude --resume <id>` in that directory.
    //
    // Only when the child is *not* in the root. A root-standing tab (the spinner's, a task-less
    // run's reviewer) was always found, and `continues` would pin its resume to the original id —
    // `continue_spec` names `conversation.id` — where the ordinary road follows `/clear` to
    // `Pane::conversation`.
    let in_worktree = cwd != root;

    // Flattened before anything else touches it. A PTY write is keystrokes, so a newline in here
    // is a second Enter that submits the tail of the prompt as its own turn — finding 8, and the
    // rule every prompt path in this codebase inherits. (Codex takes it in the argv, where a
    // newline would be harmless; flattened anyway, so the two CLIs are told the same thing.)
    let line = crate::agent_rpc::one_line(prompt);

    // Opencode is not a console, so none of the above applies to it: the TUI, the line in
    // `--prompt`, and cide's server in its configuration document (M104).
    let (program, args, env) = if opencode {
        let launch = cide_agents::harness::opencode::OPENCODE_CLI
            .tab_launch(
                &line,
                model,
                mode.unattended != cide_agents::Unattended::Ask,
                &llm,
                crate::agents::cide_hook_binary().as_deref(),
            )
            .map_err(CoreError::Io)?;
        (launch.program, launch.args, launch.env)
    } else {
        (harness.program().to_string(), args, Vec::new())
    };

    let request = crate::cmd::session::SpawnRequest {
        program,
        args,
        cwd: cwd.to_string_lossy().into_owned(),
        geometry: Geometry::default(),
        project: Some(project),
        resume: None,
        fork: None,
        continues: None,
        // **This tab is acting as the product owner, and has to be told so.** Inferring it from
        // the tree answers `Pane`, whose sentence is deliberately hedged — *you may act as one*
        // — for a case that is not this one: a conversation pane the user opened onto a task has
        // been handed work to do, and two identities in one prompt is a model guessing. Here
        // there is no second identity. cide opened this tab with a brief that tells it to judge
        // a task, merge the branch and close it or dispatch it back, which *is* the job, and a
        // system prompt calling it a bystander who could orchestrate left it arguing with itself
        // about whether it was allowed to. Reported as exactly that.
        voice: Some(voice),
        // Opencode's configuration document; nothing for a console.
        env,
        // Codex takes its opening line in the argv; claude's is typed below.
        prompt: codex.then(|| line.clone()),
        // A console has the task tools by being one; opencode's bridge needs the id handed down.
        task_tools: opencode,
    };

    let registry = app
        .try_state::<crate::state::SessionRegistry>()
        .ok_or_else(|| CoreError::Io("the session registry is going away".into()))?;

    let session =
        tauri::async_runtime::block_on(crate::cmd::session::spawn_session(app, &registry, request))
            .map_err(|error| {
                CoreError::Io(format!(
                    "could not start {} for a new tab: {error}",
                    if opencode {
                        "opencode"
                    } else {
                        harness.program()
                    }
                ))
            })?;

    // **Whether the strip moves is the one thing the two modes disagree about on screen.**
    //
    // A reviewer opens *because* a subagent finished while the user was reading something else —
    // that is the premise of the whole feature — so raising it takes the view away mid-sentence
    // to announce work nobody asked to be interrupted by. Reported as exactly that. The pane
    // still mounts behind the current tab and the review still happens; only the selection stays
    // where the user put it.
    //
    // The spinner's tab does raise, and the difference is not arbitrary: it fires only after the
    // project has been idle for a quarter of an hour with nothing running, so there is by
    // construction nothing to interrupt — and it is about to plan the next round of work, which
    // is the thing somebody coming back to the project wants in front of them.
    let behind = mode.behind;

    let pane = PaneId::new();
    let opened = state.update(|ws| {
        let open = if behind {
            cide_core::workspace::open_tab_behind
        } else {
            cide_core::workspace::open_tab
        };
        open(
            ws,
            project,
            TabKind::ClaudeFull {
                title: title.to_string(),
                // cide opened this one. See the field's doc: the mark is what makes closing the
                // tab end the child instead of parking it for a Ctrl+Shift+T nobody will press.
                ephemeral: true,
            },
            Pane {
                id: pane,
                // A console is a Claude pane; the opencode TUI is a program in a terminal, which
                // is what `pane_for` makes a re-opened opencode conversation too.
                kind: if opencode {
                    PaneKind::Shell
                } else {
                    PaneKind::Claude
                },
                // Auxiliary like every pane in a `ClaudeFull` tab: closing the last one closes
                // the tab. Only the pinned console has a pane that cannot go.
                role: PaneRole::Auxiliary,
                session: Some(session),
                conversation: None,
                conversation_since: None,
                // The id is the one `--session-id` was handed, which is what `--resume` takes.
                //
                // Not for opencode: its conversation is a `ses_…` the TUI mints and never tells
                // anybody, so there is no id to continue — and a `continues` naming cide's own
                // session id would make a restart run `opencode --session <uuid>`, which opens
                // nothing. Restored, the pane is a shell in the worktree. (M104's stated gap.)
                continues: (in_worktree && !opencode).then(|| cide_ipc::HarnessSession {
                    harness: harness.harness(),
                    id: session.to_string(),
                    cwd: cwd.clone(),
                }),
                // Written directly, as `session` is: this road never passes `pane_bind_session`.
                harness: codex.then_some(cide_ipc::Harness::Codex),
                title: format!(
                    "{name} : {}",
                    if opencode {
                        "opencode"
                    } else {
                        harness.program()
                    }
                ),
                docker: None,
                origin,
            },
        )
    });

    let tab = match opened {
        Ok(tab) => tab,
        Err(error) => {
            // The project closed between the two steps, or the mutation failed its validator.
            // Either way there is a live `claude` with no pane, which is a leak *and* a billed
            // process, so it goes now rather than at quit.
            tracing::warn!(%error, %project, "no tab for a spawned claude; killing it");
            if let Some(pty) = registry.get(session) {
                pty.kill();
            }
            return Err(error);
        }
    };

    // The pid→pane join for `cide-ide-mcp`, which `cmd::pane::pane_bind_session` would normally
    // do and which this road skips by writing `Pane::session` directly. `claude` announces
    // itself over the IDE socket with its pid, and that pid is what decides which pane an
    // `openDiff` or an `@`-mention belongs to. `ide::resolve_unbound_connections` would recover
    // it by walking ancestry — `pane_pids` reads the tree, which now names this session — but
    // both facts are in hand here, so the answer is given rather than reconstructed.
    if let (Some(servers), Some(pty)) = (
        app.try_state::<crate::ide::IdeServers>(),
        registry.get(session),
    ) && let Some(pid) = pty.child_pid()
    {
        servers.bind_pane(project, pid, pane);
    }

    if codex || opencode {
        // Already submitted, as the argv's last token (codex) or its `--prompt` (opencode).
        return Ok((session, tab));
    }
    if let (Some(bytes), Some(pty)) = (crate::agent_rpc::submit(&line), registry.get(session)) {
        // Never a plain write: the TUI detects pastes *by length*, so a prompt this long arriving
        // with its `\r` in the same `read` lands in the composer unsubmitted. This writes the
        // text now and the Enter alone once the child has reported in.
        crate::agents::type_submitted_line(app, session, &pty, bytes);
    }

    Ok((session, tab))
}
