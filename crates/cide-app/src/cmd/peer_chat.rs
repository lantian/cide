//! Command-palette-only experimental pair entry points.
use crate::workspace_state::WorkspaceState;
use cide_core::{CoreError, peer_chat as domain, workspace};
use cide_ipc::{PeerChatReceipt, PeerChatSelection, ProjectId, TabId};
use tauri::{Manager, State};

#[tauri::command(rename_all = "camelCase")]
pub fn peer_chat_open(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
) -> Result<TabId, CoreError> {
    state.update(|ws| domain::open(ws, project))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn peer_chat_configure(
    app: tauri::AppHandle,
    project: ProjectId,
    tab: TabId,
    selection: PeerChatSelection,
) -> Result<(), CoreError> {
    let state = app.state::<WorkspaceState>();
    let (main, peer, cwd) = state.update(|ws| {
        let settings = cide_core::harness_settings::effective(ws, Some(project));
        let main = workspace::tab(ws, project, tab)?
            .peer_chat
            .as_ref()
            .ok_or_else(|| CoreError::Invariant("Not a pair tab".into()))?
            .main_harness;
        for harness in [main, selection.harness] {
            validate_injections(&settings, harness)?;
        }
        domain::configure(ws, project, tab, selection)?;
        let chat = workspace::tab_mut(ws, project, tab)?
            .peer_chat
            .as_mut()
            .unwrap();
        chat.starting = true;
        chat.paused = true;
        let panes = (chat.main, chat.peer);
        let cwd = workspace::project(ws, project)?
            .roots
            .first()
            .ok_or(CoreError::NoRoots)?
            .path
            .to_string_lossy()
            .into_owned();
        Ok((panes.0, panes.1, cwd))
    })?;
    let registry = app.state::<crate::state::SessionRegistry>();
    let mut started = Vec::new();
    let outcome = async {
        for pane in [main, peer] {
            let id = crate::cmd::session::spawn_session(
                &app,
                &registry,
                crate::cmd::session::SpawnRequest {
                    program: "claude".into(),
                    harness: None,
                    args: vec![],
                    cwd: cwd.clone(),
                    geometry: cide_ipc::Geometry::default(),
                    project: Some(project),
                    resume: None,
                    resume_picker: false,
                    fork: None,
                    continues: None,
                    pane: Some(pane),
                    voice: None,
                    env: vec![],
                    prompt: None,
                    task_tools: false,
                },
            )
            .await
            .map_err(|e| CoreError::Io(e.to_string()))?;
            started.push(id);
            state.update(|ws| {
                crate::cmd::pane::bind_recorded_session(ws, &registry, project, tab, pane, id)
            })?;
            if let Some(servers) = app.try_state::<crate::ide::IdeServers>()
                && let Some(pid) = registry.get(id).and_then(|s| s.child_pid())
            {
                servers.bind_pane(project, pid, pane);
            }
        }
        if started
            .iter()
            .any(|id| registry.get(*id).is_none_or(|p| p.has_exited()))
        {
            return Err(CoreError::Io(
                "One of the paired harnesses exited during startup. Check its launch settings."
                    .into(),
            ));
        }
        state.update(|ws| {
            let chat = workspace::tab_mut(ws, project, tab)?
                .peer_chat
                .as_mut()
                .unwrap();
            chat.starting = false;
            chat.paused = false;
            workspace::bump(ws);
            Ok(())
        })
    }
    .await;
    if let Err(error) = outcome {
        crate::peer_chat::stop_pair(&app, &started);
        let _ = state.update(|ws| {
            let t = workspace::tab_mut(ws, project, tab)?;
            if let Some(chat) = &mut t.peer_chat {
                chat.selection = None;
                chat.starting = false;
                chat.paused = true;
                for p in t.tree.panes.values_mut() {
                    p.session = None;
                    p.conversation = None;
                    p.conversation_since = None;
                    p.harness_conversation = None;
                    p.continues = None;
                }
            }
            workspace::bump(ws);
            Ok(())
        });
        return Err(error);
    }
    Ok(())
}

pub(crate) fn validate_injections(
    settings: &cide_ipc::Settings,
    harness: cide_ipc::ConsoleHarness,
) -> Result<(), CoreError> {
    use cide_ipc::ConsoleHarness::*;
    let enabled = match harness {
        Claude => {
            settings.claude.cli.inject.mcp_config.enabled
                && settings.claude.cli.inject.session_id.enabled
                && settings.claude.cli.inject.resume.enabled
        }
        Codex => {
            settings.codex.cli.inject.mcp_config
                && settings.codex.cli.inject.developer_instructions
                && settings.codex.cli.inject.permissions
                && settings.codex.cli.inject.resume
                && settings.codex.cli.inject.hooks
        }
        Opencode => {
            settings.opencode.cli.inject.mcp_config
                && settings.opencode.cli.inject.instructions
                && settings.opencode.cli.inject.resume
                && settings.opencode.cli.inject.events
        }
    };
    if enabled {
        Ok(())
    } else {
        Err(CoreError::Invariant(format!(
            "Two-agent chat requires MCP, instruction and reviewer permission injections for {}. Enable them in Settings → Harness.",
            harness.program()
        )))
    }
}

#[tauri::command(rename_all = "camelCase")]
pub fn peer_chat_pause(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
    paused: bool,
) -> Result<(), CoreError> {
    if !paused {
        state.with(|ws| {
            let t = workspace::tab(ws, project, tab)?;
            let chat = t
                .peer_chat
                .as_ref()
                .ok_or_else(|| CoreError::Invariant("Not a pair tab".into()))?;
            if chat.starting
                || chat.selection.is_none()
                || t.tree.panes.values().any(|p| {
                    p.session.is_none_or(|id| {
                        app.state::<crate::state::SessionRegistry>()
                            .get(id)
                            .is_none_or(|s| s.has_exited())
                    })
                })
            {
                return Err(CoreError::Invariant(
                    "Start or restore both agents before resuming the conversation".into(),
                ));
            }
            Ok(())
        })?;
    }
    let sessions = state.with(|ws| {
        workspace::tab(ws, project, tab).map(|t| {
            t.tree
                .panes
                .values()
                .filter_map(|p| p.session)
                .collect::<Vec<_>>()
        })
    })?;
    state.update(|ws| domain::set_paused(ws, project, tab, paused))?;
    if paused {
        app.state::<crate::peer_chat::Runtime>().cancel(&sessions);
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub fn peer_chat_receipts(
    app: tauri::AppHandle,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    tab: TabId,
) -> Result<Vec<PeerChatReceipt>, CoreError> {
    state.with(|ws| workspace::tab(ws, project, tab).map(|_| ()))?;
    Ok(app.state::<crate::peer_chat::Runtime>().receipts(tab))
}

#[tauri::command(rename_all = "camelCase")]
pub async fn peer_chat_plan(
    app: tauri::AppHandle,
    project: ProjectId,
    tab: TabId,
) -> Result<Vec<cide_ipc::PaneRestore>, CoreError> {
    let snapshot = app.state::<WorkspaceState>().snapshot();
    let t = workspace::tab(&snapshot, project, tab)?;
    if t.peer_chat.is_none() {
        return Err(CoreError::Invariant("Not a pair tab".into()));
    }
    let live: Vec<_> = t
        .tree
        .panes
        .values()
        .filter(|p| {
            p.session.is_some_and(|id| {
                app.state::<crate::state::SessionRegistry>()
                    .get(id)
                    .is_some_and(|s| !s.has_exited())
            })
        })
        .map(|p| p.id)
        .collect();
    tauri::async_runtime::spawn_blocking(move || {
        Ok(crate::lifecycle::plan_restore(&snapshot, Some(project))
            .into_iter()
            .filter(|p| p.tab == tab && !live.contains(&p.pane))
            .collect())
    })
    .await
    .map_err(|e| CoreError::Io(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabled_injections_cannot_silently_remove_pairing_or_restore() {
        use cide_ipc::ConsoleHarness::*;
        let defaults = cide_ipc::Settings::default();
        for h in [Claude, Codex, Opencode] {
            validate_injections(&defaults, h).unwrap();
        }
        let mut s = defaults.clone();
        s.codex.cli.inject.resume = false;
        assert!(validate_injections(&s, Codex).is_err());
        validate_injections(&s, Claude).unwrap();
        let mut s = defaults.clone();
        s.claude.cli.inject.mcp_config.enabled = false;
        assert!(validate_injections(&s, Claude).is_err());
        let mut s = defaults.clone();
        s.opencode.cli.inject.events = false;
        assert!(validate_injections(&s, Opencode).is_err());
        let mut s = defaults;
        s.codex.cli.inject.permissions = false;
        assert!(validate_injections(&s, Codex).is_err());
    }
}
