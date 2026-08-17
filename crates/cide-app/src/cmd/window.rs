//! Window commands: tear a pane out, put it back, flip the window mode, close a window.
//!
//! All four are the same mechanism — rearrange which panes are shown where — and none of
//! them touches a child process. Nothing in this module reaches for `SessionRegistry`, and
//! that is the point: a session is owned by the process, not by the window showing it, so a
//! pane can move between windows while its child keeps writing into the vt100 mirror that
//! the next window will read its first frame from.
//!
//! Every handler is domain-first: `cide-core` decides, and this module then makes the
//! desktop agree. The order is load-bearing in both directions. A detach that fails to open
//! a window must not leave a pane in the holding map with nothing to show it, and a re-dock
//! the domain refuses must leave the window it was asked about on screen.
//!
//! What a window *is* stays here. `cide-core` names windows and says which should exist; it
//! has no idea that one is a webview.

use std::collections::BTreeSet;

use cide_core::{CoreError, workspace};
use cide_ipc::{
    PaneId, ProjectId, SessionId, TabId, WindowLabel, WindowMode, WindowRole, Workspace,
    state_key_of,
};
use tauri::{AppHandle, Manager, State};

use crate::cmd::project::Mutated;
use crate::windows;
use crate::workspace_state::WorkspaceState;

/// The pane's on-screen size in logical pixels, as the frontend last laid it out.
///
/// Optional on the wire because only the frontend can know it: Rust never measures a pane,
/// and a pane detached before its first layout has no rect to report.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub struct PaneRect {
    pub width: f64,
    pub height: f64,
}

/// Tear a pane out of its tab into a window of its own.
///
/// The pane keeps its id and its session binding, so the new window attaches to the *same*
/// `SessionId` and the child never notices. Continuity is not a DOM move — each window is a
/// separate webview and a separate JavaScript realm, so the new one builds its own terminal,
/// paints the mirror from `session.scrollback` and takes live bytes from there.
///
/// Returns the label of the window that now shows the pane.
#[tauri::command(rename_all = "camelCase")]
pub fn window_detach_pane(
    state: State<'_, WorkspaceState>,
    app: AppHandle,
    project: ProjectId,
    tab: TabId,
    pane: PaneId,
    rect: Option<PaneRect>,
) -> Result<WindowLabel, CoreError> {
    let (label, title) = state.update(|ws| {
        // Read before the move: `detach_pane` takes the pane out of the tree and into the
        // project's holding map, and the window wants the name the pane already carries.
        let title = workspace::tab(ws, project, tab)?
            .tree
            .panes
            .get(&pane)
            .map(|p| p.title.clone())
            .ok_or(CoreError::NoSuchPane(pane))?;
        Ok((workspace::detach_pane(ws, project, tab, pane)?, title))
    })?;

    let size = rect.map_or((windows::DETACHED_WIDTH, windows::DETACHED_HEIGHT), |r| {
        (r.width, r.height)
    });
    if let Err(error) = windows::create(&app, &label, &title, Some(size)) {
        // A detached pane with no window is stranded: nothing renders it, no gesture can
        // re-dock it, and its child goes on running with no way to reach or stop it. Put it
        // back rather than leave the workspace describing a window that does not exist.
        if let Err(rollback) = state.update(|ws| workspace::redock_pane(ws, &label)) {
            tracing::error!(%label, %rollback, "could not re-dock a pane whose window failed to open");
        }
        return Err(CoreError::Io(format!(
            "could not open window {label}: {error}"
        )));
    }
    // The pane took its `Awaiting:` badge with it. Both titles are now wrong — the shell is
    // still counting a pane it no longer shows, and the new window was built with the pane's
    // name and no badge — so they are recomputed together rather than patched apart.
    retitle(&app, &state.snapshot());
    Ok(label)
}

/// Put a detached pane back in its tab and close the window it was living in.
#[tauri::command(rename_all = "camelCase")]
pub fn window_redock_pane(
    state: State<'_, WorkspaceState>,
    app: AppHandle,
    label: WindowLabel,
) -> Result<Mutated, CoreError> {
    // Domain first. A refusal — an unknown label, a project that has since closed — must
    // leave the window standing, because it is the only view of a session that is still
    // running.
    let closing = state.update(|ws| workspace::redock_pane(ws, &label))?;
    windows::destroy(&app, &closing);
    // The pane is back in a tab, so its shell inherits whatever badge it was carrying.
    retitle(&app, &state.snapshot());
    Ok(Mutated { rev: state.rev() })
}

/// Switch between one window holding every project and one window per project.
///
/// The domain rewrites nothing but its window map; this then opens the shells it now names
/// and closes the ones it stopped naming. No project, tab, pane or session is touched on
/// either side of the flip, which is why it is safe with live children running.
#[tauri::command(rename_all = "camelCase")]
pub fn window_set_mode(
    state: State<'_, WorkspaceState>,
    app: AppHandle,
    mode: WindowMode,
) -> Result<Mutated, CoreError> {
    state.update(|ws| workspace::set_window_mode(ws, mode))?;
    reconcile(&app, &state)?;
    Ok(Mutated { rev: state.rev() })
}

/// Close a window, doing whatever that means for what the window is showing.
///
/// `force` reaches both shell cases, because both can discard unsaved edits: in
/// `PerProject` the window *is* the project, and in `Stacked` closing it is a quit. Without
/// it, either is refused with [`CoreError::UnsavedChanges`]; the caller shows `CloseConfirm`
/// and asks again. Re-docking a detached pane loses nothing and never consults it.
#[tauri::command(rename_all = "camelCase")]
pub fn window_close(
    state: State<'_, WorkspaceState>,
    app: AppHandle,
    label: WindowLabel,
    force: bool,
) -> Result<Mutated, CoreError> {
    let ws = state.snapshot();
    let Some(role) = ws.windows.get(&label).cloned() else {
        // A window the workspace does not name should not exist, so closing it is the
        // repair rather than an error. That is the state a pruning leaves behind:
        // `close_project` and `close_tab` drop the windows detached from what they close,
        // and the OS window outlives the entry that named it.
        windows::destroy(&app, &label);
        return Ok(Mutated { rev: ws.rev });
    };

    match role {
        // Re-dock, never discard. Discarding would strand a live session with no view and
        // no way back: the pane is the only thing bound to that `SessionId`, so dropping it
        // leaves a child running that nothing can reach, show or stop.
        //
        // # Every way a detached-pane window can close, and what each one does
        //
        // The window now draws minimize / maximize / close in its pane title bar (M12), so
        // this is no longer reachable only from the re-dock button. All five routes are
        // written out because the guarantee — **the session survives and the pane ends up
        // somewhere the user can reach** — has to hold on every one of them, and four of the
        // five do not come through this function.
        //
        // 1. **The close control, or `Close window` in the pane menu.** Both are
        //    `data-window-button="close"`, which `WindowFrame` turns into `window.close()`;
        //    that raises `CloseRequested`, and [`intercept_close`] takes it over and re-docks.
        //    Not this path — but the same `redock_pane`, deliberately, so the two cannot
        //    diverge.
        // 2. **Alt+F4, or the compositor's own close.** Identical to (1): the event loop
        //    delivers `CloseRequested` and [`intercept_close`] answers it. This is why the
        //    interception exists at all — a detached window has no unsaved-work dialog to
        //    raise, only a pane to put back.
        // 3. **This command.** Reachable from `windows.close(label)`; the arm below.
        // 4. **Its session is busy.** Nothing here consults liveness and nothing should: a
        //    re-dock interrupts no turn, kills no child and loses no transcript — the pane
        //    reappears in its tab still streaming. A confirmation on this gesture would be a
        //    dialog that always says "nothing will be lost", which is how a user learns to
        //    dismiss the ones that matter.
        // 5. **Its home tab has been closed.** `redock_pane` falls back to the project's
        //    console tab, which cannot be closed, so there is always somewhere to land; the
        //    saved `DockAnchor` is checked with `can_restore` first and simply does not match
        //    the console's tree, so the pane lands beside its focused pane instead. If the
        //    whole *project* has gone, `close_project` already pruned this window's role and
        //    `reconcile` destroyed the window — the early return above is what that reaches,
        //    and nothing is stranded because there is nothing left to strand it in.
        //
        // Quitting with detached windows open is the one case that deliberately does **not**
        // re-dock: [`quit`] destroys windows rather than closing them, so no `CloseRequested`
        // is raised, the pane stays in `project.detached`, and `restore_windows` reopens its
        // window on the next launch with `plan_restore` offering to resume the conversation.
        // Re-docking on the way out would silently rearrange the layout the user left.
        WindowRole::DetachedPane { .. } => {
            let closing = state.update(|ws| workspace::redock_pane(ws, &label))?;
            windows::destroy(&app, &closing);
            // The pane changed windows, so both counts moved even though the waiting set did
            // not: the window that was announcing it is gone and the shell now shows it.
            // `window_redock_pane` retitles for exactly this reason and these two must not
            // diverge — that is the whole claim of route 3 above.
            retitle(&app, &state.snapshot());
        }

        WindowRole::DetachedTab { .. } => {
            // Nothing creates one of these yet, and `cide-core` has no `redock_tab` to undo
            // it with. Refusing is the honest answer: destroying the window would take the
            // tab's panes off screen with no gesture that brings them back.
            return Err(CoreError::Invariant(format!(
                "window {label} shows a detached tab, which cannot yet be re-docked"
            )));
        }

        WindowRole::Shell { projects, .. } => match ws.settings.window_mode {
            // The window *is* the project here, so closing it closes the project — and
            // `close_project` drops the project's detached windows with it. Closing the
            // last one leaves the workspace naming no windows at all, `reconcile` destroys
            // the last OS window, and the runtime turns that into the quit it should be.
            WindowMode::PerProject => {
                state.update(|ws| {
                    for id in &projects {
                        workspace::close_project(ws, *id, force)?;
                    }
                    Ok(())
                })?;
                reconcile(&app, &state)?;
            }
            // In stacked mode the shell is the application: there is exactly one, and the
            // workspace goes on naming it so the next launch reopens it with its projects.
            // Closing it is therefore a quit, and a quit has to take the detached-pane
            // windows with it — left behind they would hold the process open showing panes
            // whose shell is gone.
            //
            // No `close_project` runs on this path, so there is no domain refusal to inherit
            // and the check is made here instead. `force` has to mean the same thing through
            // every close or it means nothing: a caller that omits it must not be able to
            // reach a wider gesture that discards more.
            //
            // This closes the *command* path only. A close delivered by the window manager
            // to a shell — Alt+F4, the compositor's own button — never reaches a command and
            // is still unguarded; see `intercept_close`, and the note in
            // `app_quit_requested`.
            WindowMode::Stacked => {
                if !force {
                    let tabs = workspace::unsaved_tabs(&ws, None);
                    if !tabs.is_empty() {
                        return Err(CoreError::UnsavedChanges { tabs });
                    }
                }
                quit(&app)
            }
        },
    }

    Ok(Mutated { rev: state.rev() })
}

/// Record whether one session has finished processing and is waiting for the user.
///
/// The frontend decides, because the decision needs history that `SessionState` does not
/// carry: `Idle` is both "started and never asked anything" and "just finished a turn", and
/// only the sequence of `cide://session-state` events tells the two apart. See
/// `ui/src/panes/awaitingRule.ts`, which is the whole rule and is unit-tested on its own.
///
/// This end owns the two halves the frontend cannot do:
///
/// * **the arithmetic**, which needs the workspace — which pane is shown in which OS window;
/// * **the title**, because a webview cannot rename its own OS window.
///
/// Idempotent and per session. Every window observes the same broadcast and will report the
/// same answer; a window that has only just opened has observed nothing and reports nothing,
/// which is what stops a fresh window clearing every marker in the app.
///
/// Not `async`/`spawn_blocking`: nothing here touches the filesystem or the network. It takes
/// the workspace lock and calls `set_title`, which is what `window_close` and `window_set_mode`
/// beside it already do.
#[tauri::command(rename_all = "camelCase")]
pub fn window_set_awaiting(
    state: State<'_, WorkspaceState>,
    app: AppHandle,
    session: SessionId,
    awaiting: bool,
) -> Result<(), CoreError> {
    // Second of the three attention-chain lines — see the note in `hooks.rs`. Logged BEFORE the
    // early return, because "the webview reported and the set did not move" is a different
    // failure from "the webview never reported" and the two are indistinguishable afterwards.
    tracing::info!(%session, awaiting, "attention: the webview reported");
    if !windows::set_awaiting(session, awaiting) {
        // Nothing moved. Retitling every window and re-broadcasting on a repeat report would
        // put one round trip per window per tool call in front of the user's keystrokes.
        return Ok(());
    }
    retitle(&app, &state.snapshot());
    crate::emit::session_awaiting(&app, windows::awaiting_sessions());
    Ok(())
}

/// The sessions this process currently believes are waiting for the user.
///
/// A **pull**, and it is what makes the claim on [`crate::emit::session_awaiting`] true.
/// That event is a *change* notification — it is emitted when the set moves — so a window
/// that opened after the last move has heard nothing at all and cannot derive the answer
/// either, because deriving it needs a history of `cide://session-state` transitions that
/// happened before it existed.
///
/// That window is not hypothetical and it is the worst possible one to get wrong:
/// [`window_detach_pane`] builds it, [`retitle`] has already put `Awaiting: 1` in its OS
/// title before its first pane renders, and the pane the user tore out *because* it was
/// waiting would show no marker. The task bar and the pane would disagree, in the one window
/// where the disagreement is most visible.
///
/// Called once per window, from the first `useAwaiting`. Not `async`/`spawn_blocking`: it
/// takes one uncontended mutex and copies a handful of ids, exactly as [`window_list`]
/// beside it copies the window map.
#[tauri::command(rename_all = "camelCase")]
pub fn window_awaiting_sessions() -> Vec<SessionId> {
    windows::awaiting_sessions()
}

/// Recompute every window's title **and its urgency hint** from the workspace, the waiting
/// set and the focus.
///
/// Called after a report, after any mutation that moves a pane between windows, and on every
/// `WindowEvent::Focused` — all three change the answer. A detach has to take the
/// `Awaiting: 1` out of the shell's title and put it in the new window's; a focus change has
/// to move the hint, because a window the user is in must not ask for them and a window they
/// have just left must, if it is still holding a finished turn they have not read.
///
/// **Everything this computes is a level, never an edge.** That is the correction to the
/// reported bug and the property to preserve: nothing here remembers what it announced last
/// time, so there is no state to get stuck and no "it already fired" to be wrong about. Every
/// call recomputes both surfaces for every window from the current facts, and the cost of the
/// ones that did not move is a `set_title` with the string it already has. See
/// [`windows::announce`] for why the hint could not stay an edge.
///
/// The two go together deliberately and must stay together. They are one fact — *does this
/// window hold a session that wants the user* — asked once and applied to the two surfaces the
/// desktop offers, and the reason the title alone was not enough is that a task bar is not
/// obliged to draw it (see [`windows::demand_attention`], which is the half that was missing
/// and is the literal content of "no cide title change in taskbar"). Computing the count twice,
/// in two functions, is how a window ends up flashing with `Awaiting: 0` in its title.
///
/// **Which sessions count toward which window** is the question the user's wording leaves
/// open, and the answer is *the sessions that window is showing*. A title is read in a task
/// switcher, and a task switcher's whole job is to say which window to go to — so a badge on a
/// window that cannot show you the session is a badge that sends you to the wrong place. In
/// practice:
///
/// * a **detached pane** window counts its one pane, so its badge is 0 or 1;
/// * a **shell** counts every pane of every project it holds, across all of that project's
///   tabs — a background tab is still in that window, and one of the two situations the user
///   described is precisely a session finishing in a tab they are not looking at;
/// * a pane that has been torn out is counted by *its* window and **not** by the shell it came
///   from, so one waiting session is announced by exactly one window and the count over all
///   windows is the truth rather than a multiple of it.
pub(crate) fn retitle(app: &AppHandle, ws: &Workspace) {
    for (label, role) in &ws.windows {
        let sessions = sessions_of(ws, role);
        let count = windows::awaiting_among(&sessions);
        // Third of the three attention-chain lines — see the note in `hooks.rs`. `debug`, not
        // `info`: this runs on every workspace mutation, so at `info` it would drown the two
        // above, which fire once per turn. What it answers is the last question — whether the
        // sessions this window is judged to hold are the ones the awaiting set knows about.
        tracing::debug!(
            %label,
            count,
            shown = sessions.len(),
            "attention: retitle"
        );
        // One call for both surfaces, so they cannot be computed from two different readings
        // of the same facts. The focus is read per window: "is the user here" is a question
        // about one OS window, not about the application.
        let say = windows::announce(&title_for(ws, role), count, windows::is_focused(label));
        windows::set_title(app, label, &say.title);
        windows::demand_attention(app, label, say.attention);
    }
}

/// The desktop moved the focus into or out of one of our windows.
///
/// Both directions, and the `false` one is the fix: a window the user has just walked away
/// from, still holding a session that finished its turn and has not been read, has to start
/// asking for them. Nothing else will ever ask on its behalf — the waiting count does not
/// move again for a session that is already waiting, so before this the only opportunity to
/// raise the hint was the instant the turn ended, and if the user happened to be in the
/// window at that instant the request was dropped by the window manager and never remade.
/// That is "i saw it once only": the first turn is the one you start before walking away, and
/// every turn after it ends with you sitting right there.
///
/// A whole [`retitle`] rather than a poke at one window's hint, because the answer is a level
/// over facts this does not own and the cheapest correct thing is to recompute it. A focus
/// change is a human-scale event; the work is a mutex, a few string builds and a `set_title`
/// per window with the string it already has.
///
/// `try_state` because window events are delivered from the moment a window exists, which can
/// precede `manage` during startup. No workspace means no windows to title.
pub(crate) fn focus_changed(app: &AppHandle, label: &WindowLabel, has_focus: bool) {
    windows::set_focused(label, has_focus);
    let Some(state) = app.try_state::<WorkspaceState>() else {
        return;
    };
    retitle(app, &state.snapshot());
}

/// The sessions one window is showing.
///
/// A set, so two panes mirroring one child — which is a real gesture in this app,
/// `claude.mirror` — count as the one conversation they are rather than as two.
fn sessions_of(ws: &Workspace, role: &WindowRole) -> BTreeSet<SessionId> {
    let mut out = BTreeSet::new();
    match role {
        WindowRole::Shell { projects, .. } => {
            for id in projects {
                let Ok(project) = workspace::project(ws, *id) else {
                    continue;
                };
                // Tabs only. `project.detached` is deliberately skipped: those panes have
                // windows of their own, and counting them here would announce one waiting
                // session from two places.
                for tab in &project.tabs {
                    out.extend(tab.tree.panes.values().filter_map(|p| p.session));
                }
            }
        }
        WindowRole::DetachedPane { project, pane, .. } => {
            if let Ok(project) = workspace::project(ws, *project)
                && let Some(session) = project.detached.get(pane).and_then(|p| p.session)
            {
                out.insert(session);
            }
        }
        WindowRole::DetachedTab { project, tab } => {
            if let Ok(tab) = workspace::tab(ws, *project, *tab) {
                out.extend(tab.tree.panes.values().filter_map(|p| p.session));
            }
        }
    }
    out
}

/// What goes in a window's title bar, before the awaiting badge.
fn title_for(ws: &Workspace, role: &WindowRole) -> String {
    match role {
        WindowRole::Shell { projects, .. } => shell_title(ws, projects),
        // Derived from the workspace on every retitle rather than remembered from the detach.
        // The pane's title is not immutable — `claude : bash` becomes `claude : claude` when a
        // pane's kind changes — and a remembered string would go stale with nothing to correct
        // it.
        WindowRole::DetachedPane { project, pane, .. } => workspace::project(ws, *project)
            .ok()
            .and_then(|p| p.detached.get(pane))
            .map(|p| p.title.clone())
            .unwrap_or_else(|| "cide".to_string()),
        // Nothing creates one of these yet (see `window_close`), and a `Tab` carries no title
        // of its own — the strip labels it from its `kind`. The project name is the honest
        // answer until there is a gesture that makes one.
        WindowRole::DetachedTab { project, .. } => workspace::project(ws, *project)
            .map(|p| p.name.clone())
            .unwrap_or_else(|_| "cide".to_string()),
    }
}

/// Bring the window showing one pane to the front, and say which window that was.
///
/// The frontend's half of *send lines to Claude* can activate a tab and focus a pane — both
/// are ordinary domain mutations — and then stops at the edge of its own window. A webview
/// cannot raise an OS window, so a mention that lands in a pane living in a **detached
/// window**, or in the shell while the gesture was made from a detached editor, arrives
/// somewhere the user is not looking with nothing to say so. That is the same "did anything
/// happen?" complaint the send itself already answered once.
///
/// `None` rather than an error when no window shows the pane. It is not a failure of this
/// command: the pane closed between the send and the reveal, or its project did, and the
/// caller words that for the user far better than `no such pane: <uuid>` would. An error is
/// reserved for something the caller can act on, and there is nothing to act on here.
///
/// **Which window shows a pane** is arithmetic over the workspace and belongs on this side
/// for the reason [`retitle`] gives about badges: the answer is "the window you would have to
/// go to", and only the workspace knows that. The frontend's mirror could answer it too and
/// would answer it from a snapshot that may be one event behind a detach.
///
/// Not `async`/`spawn_blocking`: no filesystem, no network. One uncontended lock and two
/// window-manager calls, exactly as [`window_set_awaiting`] beside it takes one lock and sets
/// a title.
#[tauri::command(rename_all = "camelCase")]
pub fn window_reveal_pane(
    state: State<'_, WorkspaceState>,
    app: AppHandle,
    project: ProjectId,
    pane: PaneId,
) -> Option<WindowLabel> {
    let label = window_showing(&state.snapshot(), project, pane)?;
    windows::raise(&app, &label);
    Some(label)
}

/// The window a pane is on screen in, or `None` when nothing shows it.
///
/// **Torn-out windows are asked first, and that is not a style choice.** A pane detached into
/// its own window leaves its tab's tree, so a shell would decline it whatever the order — but
/// a detached *tab* keeps its tab in `project.tabs`, so the shell that owns the project still
/// answers `holds` for every pane in it. Scanning one pass over `ws.windows` therefore hands
/// the answer to whichever role `IndexMap` happens to name first, and on a fresh workspace
/// that is always the shell: the reveal would raise a window that is not drawing the pane, and
/// the `DetachedTab` arm — written out precisely so a future detach inherits a working reveal
/// — would never be reached to be noticed.
fn window_showing(ws: &Workspace, project: ProjectId, pane: PaneId) -> Option<WindowLabel> {
    let torn_out = ws.windows.iter().find_map(|(label, role)| match role {
        WindowRole::DetachedPane { pane: shown, .. } if *shown == pane => Some(label.clone()),
        // Nothing creates one of these yet (see `window_close`), and it is written out rather
        // than folded into the wildcard so that whatever does create one inherits a reveal
        // that works instead of one that raises the shell the tab came from.
        WindowRole::DetachedTab {
            project: owner,
            tab,
        } if *owner == project => workspace::tab(ws, *owner, *tab)
            .is_ok_and(|t| t.tree.panes.contains_key(&pane))
            .then(|| label.clone()),
        _ => None,
    });
    if torn_out.is_some() {
        return torn_out;
    }

    ws.windows.iter().find_map(|(label, role)| match role {
        // The project is checked as well as the pane, because a shell holding several
        // projects would otherwise claim a pane belonging to any of them. `holds` then asks
        // the tabs, so a shell whose project has *torn the pane out* declines and the pass
        // above has already answered.
        WindowRole::Shell { projects, .. } if projects.contains(&project) => {
            holds(ws, project, pane).then(|| label.clone())
        }
        _ => None,
    })
}

/// Whether one of the project's tabs holds this pane.
fn holds(ws: &Workspace, project: ProjectId, pane: PaneId) -> bool {
    workspace::project(ws, project)
        .is_ok_and(|p| p.tabs.iter().any(|t| t.tree.panes.contains_key(&pane)))
}

/// Every window the workspace names, and what each one shows.
///
/// The workspace rather than the desktop: this is the list the frontend reasons about, and
/// a live OS window missing from it is something for [`reconcile`] to clear up, not a row
/// to draw.
#[tauri::command(rename_all = "camelCase")]
pub fn window_list(state: State<'_, WorkspaceState>) -> Vec<(WindowLabel, WindowRole)> {
    state.snapshot().windows.into_iter().collect()
}

/// Answer a close the window manager delivered rather than a command.
///
/// Returns whether this took the close over, in which case the caller must prevent the
/// native one. Only a detached pane is taken over. Letting that window close natively
/// leaves the pane in its project's holding map with a live child, nothing rendering it and
/// no gesture that can reach it — the app draws no window list, and the entry outlives the
/// process in `workspace.json`, so the pane is missing from every later launch too.
///
/// A shell keeps the native close. What that gesture means for a shell — close the project,
/// or quit — is a decision for [`window_close`]'s caller, and answering it here would make
/// Alt+F4 delete a project the user expected to find again.
///
/// That is also the one hole left in the unsaved-work guard, and it is named rather than
/// papered over: [`window_close`] refuses unsaved edits, but Alt+F4 on a shell never reaches
/// it. Closing the hole means `prevent_close` here plus a new event asking the frontend
/// whether to proceed — and a veto whose answer never arrives is an app that cannot be quit,
/// which is a worse failure than the one it fixes. See `app_quit_requested`.
pub fn intercept_close(app: &AppHandle, label: &WindowLabel) -> bool {
    // Before `manage` there is no workspace to consult, and so no pane to strand.
    let Some(state) = app.try_state::<WorkspaceState>() else {
        return false;
    };
    let snapshot = state.snapshot();
    let role = snapshot.windows.get(label).cloned();

    // A shell window holding unsaved edits.
    //
    // This is the one close path with no command behind it: Alt+F4 and the compositor's own
    // button are delivered straight to the event loop, so there is no `Err` for the frontend
    // to catch and no `force` for it to pass. Without this, every other guard in the project
    // is bypassed by the most ordinary gesture there is for closing a window.
    //
    // Refusing here and telling the frontend is the whole mechanism: it raises the same
    // `CloseConfirm` the command paths raise, and discarding re-issues `window_close` with
    // `force: true`, which does have a command behind it.
    //
    // **The accepted risk, stated because it is real.** A veto whose answer never arrives is
    // a window that cannot be closed: if the webview is wedged, the dialog never paints and
    // Alt+F4 stops working. The escape is to signal the process, which runs the same shutdown
    // ladder as a clean quit and flushes the workspace.
    //
    // That is the deliberate direction. An unclosable window is recoverable in one command;
    // a discarded buffer is not recoverable at all, and this project has chosen the
    // recoverable failure every other time the question has come up — a cancelled diff
    // rejects rather than accepts, a dismissed diff tab rejects rather than writes. A
    // heuristic escape ("honour a second Alt+F4") was considered and rejected: pressing twice
    // is something people do out of habit, so it would discard buffers on a reflex.
    if let Some(WindowRole::Shell { projects, .. }) = &role {
        let unsaved: Vec<_> = projects
            .iter()
            .flat_map(|p| workspace::unsaved_tabs(&snapshot, Some(*p)))
            .collect();
        if !unsaved.is_empty() {
            crate::emit::close_blocked(app, label, unsaved);
            return true;
        }
    }

    if !matches!(role, Some(WindowRole::DetachedPane { .. })) {
        return false;
    }

    let app = app.clone();
    let label = label.clone();
    // Off the event-loop thread. Destroying a window from inside the delivery of that
    // window's own close event re-enters the loop delivering it; raised from a worker, the
    // destroy arrives as a queued message the loop handles after this handler has returned
    // and `prevent_close` has been honoured.
    tauri::async_runtime::spawn_blocking(move || {
        let Some(state) = app.try_state::<WorkspaceState>() else {
            return;
        };
        match state.update(|ws| workspace::redock_pane(ws, &label)) {
            Ok(closing) => {
                windows::destroy(&app, &closing);
                // Routes 1 and 2 — the close control this window now draws, and Alt+F4 — end
                // here rather than in `window_close`, so the retitle has to be here too. A
                // waiting pane put back by either of them would otherwise leave its shell
                // wearing the title it had while the pane was somebody else's: no badge, for
                // a session that is still waiting and is now in one of its tabs. Nothing
                // else would correct it until the next report moved the set.
                retitle(&app, &state.snapshot());
            }
            Err(error) => {
                // The pane is no longer re-dockable — its project or its home tab has gone,
                // and the entry naming this window went with them. Nothing is stranded by
                // letting the window go.
                tracing::error!(%label, %error, "could not re-dock a pane whose window was closed");
                windows::destroy(&app, &label);
            }
        }
    });
    true
}

// --- internals ---------------------------------------------------------------------

/// Make the set of OS windows match the set the workspace names.
///
/// Creates before it destroys. The other order leaves the process with no window for an
/// instant, and the runtime reads the last window going away as a request to exit.
///
/// Only shells are created. A `DetachedPane` the workspace names but the desktop has lost
/// is a stranded pane, and reopening it as a side effect of a mode flip would be a surprise
/// — the mode says nothing about detached windows. Equally, only windows the workspace has
/// stopped naming are destroyed, which is what leaves detached panes strictly alone: a mode
/// flip never removes them from the map.
/// Bring the desktop into line with what the workspace says.
///
/// Callable from outside this module because closing a project or a tab also invalidates
/// windows: both prune the roles that named what they closed, and without a reconcile the
/// OS window outlives the entry. What is left behind is worse than an empty frame — it is a
/// window whose pane no longer exists, showing nothing, with a live child behind it that
/// nothing can re-attach to and nothing will reap.
pub(crate) fn reconcile(app: &AppHandle, state: &WorkspaceState) -> Result<(), CoreError> {
    let ws = state.snapshot();
    for (label, role) in &ws.windows {
        let WindowRole::Shell { projects, .. } = role else {
            continue;
        };
        if app.get_webview_window(label.as_str()).is_some() {
            continue;
        }
        // Reported rather than logged: half a flip is worse than none, and at this point
        // nothing has been destroyed, so the caller still has every window it started with.
        windows::create(app, label, &shell_title(&ws, projects), None)
            .map_err(|e| CoreError::Io(format!("could not open window {label}: {e}")))?;
    }

    // Which windows are doomed is decided under the lock, together with the enumeration it
    // is compared against. Reading the workspace and listing the OS windows as two separate
    // steps leaves a gap in which a detach commits and opens its window: against the earlier
    // map that brand-new `pane:<uuid>` looks like a window the workspace never named, and
    // destroying it strands the pane in the holding map with a live child and nothing
    // showing it.
    //
    // A window created *after* the decision is simply not in the set, which is the correct
    // answer rather than a lucky one.
    let doomed: Vec<_> = state.with(|ws| {
        app.webview_windows()
            .into_iter()
            .map(|(label, window)| (WindowLabel(label), window))
            .filter(|(label, _)| minted_here(label) && !ws.windows.contains_key(label))
            .collect()
    });

    // Outside the lock: destroying a window runs its close handler, which takes the lock.
    for (label, window) in doomed {
        if let Err(error) = window.destroy() {
            tracing::error!(%label, %error, "could not destroy a window the workspace dropped");
        }
    }

    // A mode flip redistributes projects across shells, so every surviving window is now
    // showing a different set of sessions than the title it is wearing was computed from, and
    // `create` above gave each new one a base name with no badge.
    retitle(app, &state.snapshot());
    Ok(())
}

/// Destroy every window, which the runtime turns into `ExitRequested` once the last one
/// goes — and that is where the workspace is written and the children are killed.
fn quit(app: &AppHandle) {
    for (label, window) in app.webview_windows() {
        if let Err(error) = window.destroy() {
            tracing::error!(%label, %error, "could not destroy a window on the way out");
        }
    }
}

/// What goes in a shell window's title bar.
///
/// The project's name when the window is one project, the app's name when it holds them
/// all. We draw our own titlebar, so this string is only ever seen in task switchers and
/// window lists — the one place our header does not reach.
fn shell_title(ws: &Workspace, projects: &[ProjectId]) -> String {
    match projects {
        [only] => workspace::project(ws, *only)
            .map(|p| p.name.clone())
            .unwrap_or_else(|_| "cide".to_string()),
        _ => "cide".to_string(),
    }
}

/// Whether a Tauri window label is one this app minted.
///
/// [`reconcile`] destroys windows the workspace no longer names, and "no longer named" only
/// means anything for the three prefixes `cide-ipc` hands out. A window with any other
/// label belongs to something that is not the workspace, and closing it would be guesswork.
fn minted_here(label: &WindowLabel) -> bool {
    matches!(state_key_of(label.as_str()), "shell" | "pane" | "tab")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_labels_this_app_mints_are_reconciled() {
        assert!(minted_here(&WindowLabel::shell()));
        assert!(minted_here(&WindowLabel::detached_pane()));
        assert!(minted_here(&WindowLabel::detached_tab()));
        assert!(!minted_here(&WindowLabel("devtools".into())));
    }

    #[test]
    fn a_shell_holding_one_project_is_titled_after_it() {
        let mut ws = Workspace::default();
        let id = workspace::open_project(&mut ws, vec!["/home/dev/work/atlas".into()], None)
            .expect("a rooted project opens");

        assert_eq!(shell_title(&ws, &[id]), "atlas");
        // An empty shell is the frame with a `+` in its header, not an error.
        assert_eq!(shell_title(&ws, &[]), "cide");
    }

    /// The question the user's wording leaves open: which sessions count toward *this*
    /// window's `Awaiting: X`.
    ///
    /// A shell counts the panes in its tabs, and it stops counting a pane the moment that pane
    /// is torn out — the detached window announces it instead. Without that, one waiting
    /// session would be announced twice and the number in the task bar would be a multiple of
    /// the truth rather than the truth.
    #[test]
    fn a_torn_out_pane_is_counted_by_its_own_window_and_no_longer_by_its_shell() {
        let mut ws = Workspace::default();
        let project =
            workspace::open_project(&mut ws, vec!["/home/dev/work/atlas".into()], None).unwrap();

        // The console pane is the one pane a fresh project has. Bound by hand rather than
        // through `pane_bind_session`, which is a Tauri command and needs an `AppHandle` this
        // build cannot make.
        let tab = workspace::project(&ws, project).unwrap().tabs[0].id;
        let console = workspace::tab(&ws, project, tab).unwrap().tree.focused;
        let session = cide_ipc::SessionId::new();

        // A second pane on the same session — what `claude.mirror` builds. One conversation.
        let mirror = cide_ipc::Pane {
            id: cide_ipc::PaneId::new(),
            kind: cide_ipc::PaneKind::Claude,
            role: cide_ipc::PaneRole::Auxiliary,
            session: Some(session),
            conversation: None,
            title: "atlas : claude — mirror".into(),
        };
        let mirror_id = mirror.id;
        {
            let t = workspace::tab_mut(&mut ws, project, tab).unwrap();
            t.tree.panes.get_mut(&console).unwrap().session = Some(session);
            cide_core::layout::add_tile(&mut t.tree, console, cide_ipc::Side::After, mirror)
                .unwrap();
        }

        let shell = ws
            .windows
            .iter()
            .find_map(|(l, r)| matches!(r, WindowRole::Shell { .. }).then(|| l.clone()))
            .expect("a fresh workspace names a shell");
        let one: BTreeSet<_> = [session].into_iter().collect();
        assert_eq!(
            sessions_of(&ws, &ws.windows[&shell].clone()),
            one,
            "two panes mirroring one child are one waiting session, not two"
        );

        let detached = workspace::detach_pane(&mut ws, project, tab, mirror_id).unwrap();
        assert_eq!(
            sessions_of(&ws, &ws.windows[&detached].clone()),
            one,
            "the detached window counts the pane it is showing"
        );
        assert_eq!(
            title_for(&ws, &ws.windows[&detached].clone()),
            "atlas : claude — mirror",
            "a detached window is named after its pane, read live rather than remembered"
        );
    }

    /// The other half of the same claim, with nothing shared between the two panes.
    ///
    /// Split out because the mirror case above cannot show it: with one session the shell's
    /// set is `{s}` before the detach and `{s}` after, so a `sessions_of` that wrongly walked
    /// `project.detached` would still pass. Here the detached pane's session is its own, and
    /// counting it in both places would announce one waiting session from two windows.
    #[test]
    fn a_shell_does_not_count_the_sessions_of_panes_it_has_torn_out() {
        let mut ws = Workspace::default();
        let project =
            workspace::open_project(&mut ws, vec!["/home/dev/work/atlas".into()], None).unwrap();
        let tab = workspace::project(&ws, project).unwrap().tabs[0].id;
        let console = workspace::tab(&ws, project, tab).unwrap().tree.focused;

        let stays = cide_ipc::SessionId::new();
        let leaves = cide_ipc::SessionId::new();
        let second = cide_ipc::Pane {
            id: cide_ipc::PaneId::new(),
            kind: cide_ipc::PaneKind::Claude,
            role: cide_ipc::PaneRole::Auxiliary,
            session: Some(leaves),
            conversation: None,
            title: "atlas : claude".into(),
        };
        let second_id = second.id;
        {
            let t = workspace::tab_mut(&mut ws, project, tab).unwrap();
            t.tree.panes.get_mut(&console).unwrap().session = Some(stays);
            cide_core::layout::add_tile(&mut t.tree, console, cide_ipc::Side::After, second)
                .unwrap();
        }

        let shell = ws
            .windows
            .iter()
            .find_map(|(l, r)| matches!(r, WindowRole::Shell { .. }).then(|| l.clone()))
            .expect("a fresh workspace names a shell");
        assert_eq!(sessions_of(&ws, &ws.windows[&shell].clone()).len(), 2);

        let detached = workspace::detach_pane(&mut ws, project, tab, second_id).unwrap();
        assert_eq!(
            sessions_of(&ws, &ws.windows[&shell].clone()),
            [stays].into_iter().collect::<BTreeSet<_>>(),
            "the shell went on counting a session it can no longer show"
        );
        assert_eq!(
            sessions_of(&ws, &ws.windows[&detached].clone()),
            [leaves].into_iter().collect::<BTreeSet<_>>()
        );
    }

    /// Where *send lines to Claude* has to go to find the pane it just typed into.
    ///
    /// One test rather than three, because the claim is that the answer **moves** with the
    /// pane: the same id is shown by the shell, then by its own window, then by nothing. Three
    /// separate tests would each pass against a lookup that only ever consulted one map.
    #[test]
    fn a_pane_is_shown_by_the_window_it_is_currently_in_and_by_no_other() {
        let mut ws = Workspace::default();
        let project =
            workspace::open_project(&mut ws, vec!["/home/dev/work/atlas".into()], None).unwrap();
        let tab = workspace::project(&ws, project).unwrap().tabs[0].id;
        let console = workspace::tab(&ws, project, tab).unwrap().tree.focused;

        let second = cide_ipc::Pane {
            id: cide_ipc::PaneId::new(),
            kind: cide_ipc::PaneKind::Claude,
            role: cide_ipc::PaneRole::Auxiliary,
            session: Some(cide_ipc::SessionId::new()),
            conversation: None,
            title: "atlas : claude".into(),
        };
        let second_id = second.id;
        {
            let t = workspace::tab_mut(&mut ws, project, tab).unwrap();
            cide_core::layout::add_tile(&mut t.tree, console, cide_ipc::Side::After, second)
                .unwrap();
        }

        let shell = ws
            .windows
            .iter()
            .find_map(|(l, r)| matches!(r, WindowRole::Shell { .. }).then(|| l.clone()))
            .expect("a fresh workspace names a shell");

        assert_eq!(
            window_showing(&ws, project, second_id),
            Some(shell.clone()),
            "a pane in a tab is shown by the shell holding its project"
        );

        let detached = workspace::detach_pane(&mut ws, project, tab, second_id).unwrap();
        assert_eq!(
            window_showing(&ws, project, second_id),
            Some(detached),
            "a torn-out pane is shown by its own window, not by the shell it came from"
        );
        assert_eq!(
            window_showing(&ws, project, console),
            Some(shell),
            "the pane that stayed behind is still the shell's"
        );

        // The reveal's own failure case: the caller sent to a pane that has since gone. This
        // is why the command answers `Option` — there is nothing here for an error to add.
        assert_eq!(
            window_showing(&ws, project, cide_ipc::PaneId::new()),
            None,
            "a pane that is in no tab and no holding map is shown by nothing"
        );
    }

    /// A tab in a window of its own, which is the case a single scan gets wrong.
    ///
    /// Unlike a detached *pane*, a detached tab stays in `project.tabs` — so the shell that
    /// owns the project still answers `holds` for every pane in it, and whichever role the
    /// window map names first wins. On a fresh workspace that is always the shell, so the
    /// reveal would raise a window that is not drawing the pane at all.
    #[test]
    fn a_tab_in_its_own_window_is_shown_by_that_window_and_not_by_the_shell_it_left() {
        let mut ws = Workspace::default();
        let project =
            workspace::open_project(&mut ws, vec!["/home/dev/work/atlas".into()], None).unwrap();
        let tab = workspace::project(&ws, project).unwrap().tabs[0].id;
        let console = workspace::tab(&ws, project, tab).unwrap().tree.focused;

        let torn = cide_ipc::WindowLabel::detached_tab();
        ws.windows
            .insert(torn.clone(), WindowRole::DetachedTab { project, tab });

        assert_eq!(
            window_showing(&ws, project, console),
            Some(torn),
            "the tab's own window shows its panes, however early the map names the shell"
        );
    }

    #[test]
    fn a_shell_holding_several_projects_is_titled_after_the_app() {
        let mut ws = Workspace::default();
        let a = workspace::open_project(&mut ws, vec!["/home/dev/a".into()], None).expect("opens");
        let b = workspace::open_project(&mut ws, vec!["/home/dev/b".into()], None).expect("opens");

        assert_eq!(shell_title(&ws, &[a, b]), "cide");
    }
}
