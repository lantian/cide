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

use std::time::{Duration, Instant};

use cide_core::CoreError;
use cide_ipc::{Geometry, Pane, PaneId, PaneKind, PaneRole, ProjectId, SessionId, TabId, TabKind};
use tauri::Manager;

use crate::workspace_state::WorkspaceState;

/// Which permission mode the new child runs under.
///
/// **Both arms are unattended**, which is the fact that shapes them. These tabs are opened by
/// cide, not by a person reaching for one, and they are opened *because* nobody is watching: one
/// when a subagent finishes while the product owner is elsewhere, one when the whole project has
/// been quiet for a quarter of an hour. A prompt either of them raises is a prompt nobody
/// answers, and the child sits there holding a tab — the same silent failure
/// `AgentsConfig::permission_mode` was written for, measured on dispatched runs and no less
/// true here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabMode {
    /// Gets to work immediately.
    ///
    /// `unattended` is the project's own `AgentsConfig::unattended`, spelled the way
    /// `harness::claude` spells it for a dispatched run: one stance for every unattended child
    /// cide starts, set in one place.
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
    /// that asks is at least visible.
    Direct { unattended: cide_agents::Unattended },
    /// `--permission-mode plan`, so the child surveys before it acts.
    ///
    /// `accept` starts [`watch_for_plan`] on this session, which answers the approval that plan
    /// mode ends at. Scoped to the session id this function just minted, so no pane, run or
    /// console can be reached by it — the first of `cide_claude::plan`'s four gates, and the one
    /// that lives here.
    Plan { accept: bool },
}

/// Spawn a `claude`, open a tab holding it, and type `prompt` into it.
///
/// Answers the new session and tab once the child exists and the tree has been mutated. The
/// prompt is delivered asynchronously by [`crate::agents::type_submitted_line`], which returns
/// immediately and finishes on its own thread — so a caller is not blocked for the several
/// seconds a TUI takes to boot.
///
/// **Callable only from a thread that is neither the GTK loop nor a Tauri runtime worker**: it
/// enters the runtime with `block_on` to reach the spawn, which is `async` because forking a
/// process is not work for the webview's thread. `crate::agent_rpc`'s header states the rule.
pub(crate) fn open_with_prompt(
    app: &tauri::AppHandle,
    project: ProjectId,
    title: &str,
    prompt: &str,
    mode: TabMode,
    // Where the child stands. `None` is the project root; `Some` is a run's worktree, which is
    // what a reviewer is given — see the `cwd` note below.
    cwd: Option<std::path::PathBuf>,
) -> Result<(SessionId, TabId), CoreError> {
    let state = app
        .try_state::<WorkspaceState>()
        .ok_or_else(|| CoreError::Io("the workspace is going away".into()))?;

    // The first root, which is what a Claude pane's `cwd` is everywhere else in this app. A
    // project with none cannot host a `claude` at all, and saying so here is better than
    // forking one into whatever directory cide happens to be in.
    let (root, name) = state.with(|ws| {
        let project = cide_core::workspace::project(ws, project)?;
        let root = project
            .roots
            .first()
            .map(|root| root.path.clone())
            .ok_or(CoreError::NoRoots)?;
        Ok::<_, CoreError>((root, project.name.clone()))
    })?;

    // One flag, spelled once. `plan` wins over the project's mode where both could apply,
    // because the plan road's whole point is that the child surveys before it acts — and what it
    // is given when the plan is accepted is the CLI's own *auto mode*, which is the same freedom
    // arriving through the door that was measured to work.
    let mut args: Vec<String> = Vec::new();
    match mode {
        TabMode::Plan { .. } => {
            args.push("--permission-mode".into());
            args.push("plan".into());
        }
        TabMode::Direct { unattended } => {
            let mode = match unattended {
                cide_agents::Unattended::Auto => Some("auto"),
                cide_agents::Unattended::Bypass => Some(cide_agents::defs::BYPASS_PERMISSIONS),
                cide_agents::Unattended::Ask => None,
            };
            if let Some(mode) = mode {
                args.push("--permission-mode".into());
                args.push(mode.into());
            }
        }
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
    let cwd = cwd.filter(|path| path.is_dir()).unwrap_or(root);

    let request = crate::cmd::session::SpawnRequest {
        program: "claude".into(),
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
        voice: Some(crate::cmd::session::Voice::Acting),
        // Nothing here needs a variable of its own: the plan approval is decided in this
        // process, against a session id, rather than by a child marked for a hook to notice.
        // See `watch_for_plan`.
        env: Vec::new(),
    };

    let registry = app
        .try_state::<crate::state::SessionRegistry>()
        .ok_or_else(|| CoreError::Io("the session registry is going away".into()))?;

    let session =
        tauri::async_runtime::block_on(crate::cmd::session::spawn_session(app, &registry, request))
            .map_err(|error| {
                CoreError::Io(format!("could not start claude for a new tab: {error}"))
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
    let behind = matches!(mode, TabMode::Direct { .. });

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
                kind: PaneKind::Claude,
                // Auxiliary like every pane in a `ClaudeFull` tab: closing the last one closes
                // the tab. Only the pinned console has a pane that cannot go.
                role: PaneRole::Auxiliary,
                session: Some(session),
                conversation: None,
                conversation_since: None,
                continues: None,
                title: format!("{name} : claude"),
                docker: None,
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

    // Flattened before anything else touches it. A PTY write is keystrokes, so a newline in here
    // is a second Enter that submits the tail of the prompt as its own turn — finding 8, and the
    // rule every prompt path in this codebase inherits.
    let line = crate::agent_rpc::one_line(prompt);
    if let (Some(bytes), Some(pty)) = (crate::agent_rpc::submit(&line), registry.get(session)) {
        // Never a plain write: the TUI detects pastes *by length*, so a prompt this long arriving
        // with its `\r` in the same `read` lands in the composer unsubmitted. This writes the
        // text now and the Enter alone once the child has reported in.
        crate::agents::type_submitted_line(app, session, &pty, bytes);
    }

    if let TabMode::Plan { accept: true } = mode {
        watch_for_plan(app, session);
    }

    Ok((session, tab))
}

/// How long the watcher stays interested in one spun session.
///
/// Twenty minutes. A plan-mode turn is a survey of a repository, a plan, and then the work, and
/// the survey alone can be minutes on a large tree. The bound exists because a watcher is a
/// thread polling a mirror: it must not outlive the question it was started for, and a run that
/// has not planned in twenty minutes is not going to be helped by cide typing at it.
const PLAN_WATCH: Duration = Duration::from_secs(20 * 60);

/// How often the mirror is read while waiting.
///
/// A second, which is well inside human reaction time for a prompt nobody is watching, and cheap:
/// the poll is gated on the hook state first, so the grid walk only happens for the fraction of a
/// second the session is actually in `AwaitingPermission`.
const PLAN_POLL: Duration = Duration::from_millis(1_000);

/// The beat between keystrokes when answering a list.
///
/// A TUI reads raw and a burst arriving in one `read` is one event to it — the measurement
/// behind `crate::agents::type_submitted_line`, which is why that helper sends its Enter
/// separately. Two hundred milliseconds is far above the CLI's own frame time and far below
/// anything a person waiting would notice.
const KEY_GAP: Duration = Duration::from_millis(200);

/// Answer the plan approval on **one** session cide spawned. (M79)
///
/// # Why cide types here at all, when `AgentRegistry::stop` refuses to
///
/// That refusal is blunt and correct: a lone `\r` at a selection list **is an answer**, so a
/// graceful stop is never offered to a run in `AwaitingPermission` — it would approve the tool
/// call the stop was meant to prevent. Nothing here weakens it. What it refuses is writing
/// *blind*, at a prompt nobody has read. This reads the prompt first, through
/// `cide_claude::permission::parse`, whose own header says every rule in it is a reason to
/// refuse, and answers only a prompt it recognises with an option it found **by its words**.
///
/// The hook road would have been better and was measured not to work — `cide_claude::plan`'s
/// header carries the evidence and `cide-hook`'s guard module carries the same note.
///
/// # Bounded in every direction
///
/// One thread per spun session, for at most [`PLAN_WATCH`]; it ends at the first answer, when the
/// child exits, or at the deadline. It answers **once** and then stops, deliberately: a watcher
/// that kept going would be a standing agreement to approve whatever that conversation asks next,
/// which is a different and much larger promise than the one the setting makes.
fn watch_for_plan(app: &tauri::AppHandle, session: SessionId) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("cide-plan-accept".into())
        .spawn(move || {
            let deadline = Instant::now() + PLAN_WATCH;
            while Instant::now() < deadline {
                std::thread::sleep(PLAN_POLL);

                let Some(registry) = app.try_state::<crate::state::SessionRegistry>() else {
                    return;
                };
                let Some(pty) = registry.get(session) else {
                    return; // Forgotten: the tab was closed, which ends the child.
                };
                if pty.has_exited() {
                    return;
                }

                // The hook first, the screen second. `permission::parse` refuses anything whose
                // session is not `AwaitingPermission` — *"the hook is exact and has already
                // decided; the screen is never sufficient evidence on its own"* — so asking it
                // here as well is not a second gate but the same one, read where it is cheap:
                // a grid walk takes the mirror's lock, and doing that every second for twenty
                // minutes on a session that is quietly working would be a cost for nothing.
                let Some(hooks) = app.try_state::<crate::hooks::HookServer>() else {
                    return;
                };
                let state = hooks.state(session);
                if state != cide_ipc::SessionState::AwaitingPermission {
                    continue;
                }

                let screen = pty.capture_screen();
                let Some(prompt) = cide_claude::permission::parse(&screen, state) else {
                    // A prompt this cannot read is left for a person, which is the honest
                    // degradation: the tab is on screen with its pane marked, exactly as it
                    // would be with `autoSpinAcceptPlan` off.
                    continue;
                };
                let Some(number) = cide_claude::plan::approval(&prompt) else {
                    // Read, and not the plan question — a file write, a command, a fetch. Left
                    // alone, and the watcher goes on waiting for the one it was started for.
                    continue;
                };
                tracing::info!(
                    %session, number,
                    "answering a spun run's plan approval; nobody is at the keyboard"
                );
                // Moved to and confirmed, not addressed by number: the digit alone was measured
                // to do nothing on this prompt — `cide_claude::plan::keystrokes` carries it —
                // and the arrow's spelling depends on the cursor mode the program asked for,
                // which the mirror knows and nothing else does.
                //
                // Paced, and **not** through `type_submitted_line`: that helper is for a line of
                // text owed one Enter, and this is a sequence of keys where each one must be read
                // before the next arrives. A TUI still painting takes a burst as one chunk.
                let keys =
                    cide_claude::plan::keystrokes(prompt.selected, number, screen.info.app_cursor);
                for key in keys {
                    pty.write(key);
                    std::thread::sleep(KEY_GAP);
                }
                return;
            }
            tracing::debug!(%session, "no plan appeared before the watcher gave up");
        });
    if let Err(error) = spawned {
        // The tab still works and still plans; it simply waits for a person, which is what
        // `autoSpinAcceptPlan: false` asks for anyway. Said out loud because the difference is
        // otherwise invisible — a tab sitting at a prompt looks the same either way.
        tracing::warn!(%error, %session, "no thread to accept this run's plan; it will wait");
    }
}
