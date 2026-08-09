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

use cide_core::{CoreError, workspace};
use cide_ipc::{
    PaneId, ProjectId, TabId, WindowLabel, WindowMode, WindowRole, Workspace, state_key_of,
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
        WindowRole::DetachedPane { .. } => {
            let closing = state.update(|ws| workspace::redock_pane(ws, &label))?;
            windows::destroy(&app, &closing);
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
    if !matches!(
        state.snapshot().windows.get(label),
        Some(WindowRole::DetachedPane { .. })
    ) {
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
            Ok(closing) => windows::destroy(&app, &closing),
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

    #[test]
    fn a_shell_holding_several_projects_is_titled_after_the_app() {
        let mut ws = Workspace::default();
        let a = workspace::open_project(&mut ws, vec!["/home/dev/a".into()], None).expect("opens");
        let b = workspace::open_project(&mut ws, vec!["/home/dev/b".into()], None).expect("opens");

        assert_eq!(shell_title(&ws, &[a, b]), "cide");
    }
}
