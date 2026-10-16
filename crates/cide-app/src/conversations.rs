//! Companion conversation reads and explicit actions. All callers run on the blocking pool.
use crate::{state::SessionRegistry, workspace_state::WorkspaceState};
use cide_core::conversations::{self as domain, JsonlReader, Transcript};
use cide_ipc::{
    Harness, HarnessSession, Pane, PaneId, PaneKind, PaneRole, ProjectId, SessionId, SessionState,
    TabKind, conversations::*, sessions::SessionRecord,
};
use parking_lot::Mutex;
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};

type Key = (ProjectId, String);
type CachedAction = (String, String, Result<ConversationResponse, String>);
#[derive(Default)]
pub(crate) struct Runtime {
    caches: Mutex<HashMap<Key, Arc<Mutex<Cache>>>>,
    actions: Mutex<VecDeque<CachedAction>>,
    pub consoles: dashmap::DashMap<SessionId, Arc<crate::opencode_console::Console>>,
}
struct Cache {
    seed: String,
    path: Option<PathBuf>,
    jsonl: JsonlReader,
    export: Transcript,
    checked: Option<Instant>,
    available: bool,
    error: Option<String>,
    state: Option<SessionState>,
}
impl Default for Cache {
    fn default() -> Self {
        Self {
            seed: uuid::Uuid::new_v4().to_string(),
            path: None,
            jsonl: JsonlReader::default(),
            export: Transcript::default(),
            checked: None,
            available: false,
            error: None,
            state: None,
        }
    }
}

fn root(app: &AppHandle, project: ProjectId) -> Result<PathBuf, String> {
    let ws = app
        .try_state::<WorkspaceState>()
        .ok_or("cide is shutting down")?;
    crate::tasks_state::project_root(&ws, project).map_err(|e| e.to_string())
}
fn record(app: &AppHandle, project: ProjectId, id: &str) -> Result<SessionRecord, String> {
    let root = root(app, project)?;
    let sessions = crate::sessions_state::handle(app).ok_or("cide is shutting down")?;
    sessions
        .record(&root, id)
        .or_else(|| {
            let session = id.parse::<SessionId>().ok()?;
            sessions
                .records(&root)
                .into_iter()
                .filter(|r| r.session == Some(session))
                .max_by_key(|r| r.last_seen_unix_ms)
        })
        .ok_or_else(|| "This conversation is no longer in this project's history.".into())
}

/// Conversation identity always wins over an old PTY handle retained after /clear or /resume.
fn live(app: &AppHandle, record: &SessionRecord) -> Option<SessionId> {
    let registry = app.try_state::<SessionRegistry>()?;
    let matches = |session| {
        if registry.get(session).is_none_or(|s| s.has_exited()) {
            return false;
        }
        if registry.codex_cleared(session) {
            return !record.known && record.id == session.to_string();
        }
        if let Some((id, _)) = registry.conversation_of(session) {
            return id.to_string() == record.id;
        }
        if let Some(native) = registry.native_conversation(session) {
            return native.harness == record.harness && native.id == record.id;
        }
        record.session == Some(session)
    };
    record
        .session
        .filter(|s| matches(*s))
        .or_else(|| registry.ids().into_iter().find(|s| matches(*s)))
}

fn policy_allows_resume(settings: &cide_ipc::Settings, harness: Harness) -> bool {
    match harness {
        Harness::Claude => settings.claude.cli.inject.resume.enabled,
        Harness::Codex => settings.codex.cli.inject.resume,
        Harness::Opencode => settings.opencode.cli.inject.resume,
        Harness::Qwen | Harness::Mimo => true,
    }
}

fn resume_enabled(app: &AppHandle, project: ProjectId, harness: Harness) -> bool {
    app.try_state::<WorkspaceState>().is_some_and(|workspace| {
        workspace.with(|ws| {
            policy_allows_resume(
                &cide_core::harness_settings::effective(ws, Some(project)),
                harness,
            )
        })
    })
}

fn summary(app: &AppHandle, project: ProjectId, record: &SessionRecord) -> ConversationSummary {
    let mut session = live(app, record);
    let mut state = session.and_then(|s| crate::emit::last_session_states().get(&s).copied());
    if state.is_none() {
        state = record
            .session
            .and_then(|s| crate::emit::last_session_states().get(&s).copied())
            .filter(|s| matches!(s, SessionState::Exited { .. }));
    }
    let mut owned = false;
    if let Some(agents) = app.try_state::<Arc<crate::agents::AgentRegistry>>() {
        if let Some(run) = record
            .run
            .and_then(|id| agents.runs_for(project).into_iter().find(|r| r.run == id))
        {
            use cide_ipc::RunState;
            if run.session.is_some_and(|s| agents.owns_session(s))
                && (session.is_none() || session == run.session)
            {
                owned = true;
                session = run.session;
                state = Some(match run.state {
                    RunState::Running => SessionState::Busy,
                    RunState::AwaitingPermission => SessionState::AwaitingPermission,
                    RunState::Paused { .. } => SessionState::Paused,
                    RunState::Idle => SessionState::AwaitingInput,
                    _ => SessionState::Spawning,
                });
            }
        }
        owned |= session.is_some_and(|s| agents.owns_session(s));
    }
    ConversationSummary {
        project,
        id: record.id.clone(),
        harness: record.harness,
        title: record.name.clone().unwrap_or_else(|| record.title.clone()),
        preview: record.prompt.clone().unwrap_or_default(),
        updated_unix_ms: record.last_seen_unix_ms,
        session,
        state,
        writable: session.is_some() && !owned,
        can_resume: session.is_none()
            && record.known
            && record.cwd.is_dir()
            && resume_enabled(app, project, record.harness),
    }
}

pub(crate) fn handle(
    app: &AppHandle,
    request: ConversationRequest,
) -> Result<ConversationResponse, String> {
    let sessions = crate::sessions_state::handle(app).ok_or("cide is shutting down")?;
    let runtime = &sessions.conversations;
    match request {
        ConversationRequest::List { project } => {
            let root = root(app, project)?;
            // Fill persisted titles and previews through the desktop journal's existing reader.
            let _ = crate::sessions_state::list(app, project);
            let conversations = sessions
                .records(&root)
                .iter()
                .filter(|r| r.known || live(app, r).is_some())
                .map(|r| summary(app, project, r))
                .collect();
            Ok(ConversationResponse::List { conversations })
        }
        ConversationRequest::Read {
            project,
            id,
            follow_session,
            generation,
            before,
            after,
        } => {
            let id = follow_session
                .and_then(|s| {
                    let registry = app.try_state::<SessionRegistry>()?;
                    registry
                        .conversation_of(s)
                        .map(|(id, _)| id.to_string())
                        .or_else(|| registry.native_conversation(s).map(|c| c.id))
                })
                .unwrap_or(id);
            let record = record(app, project, &id)?;
            let conversation = summary(app, project, &record);
            let cache = cache(runtime, project, &record.id);
            let mut cache = cache.lock();
            refresh(app, project, &record, &mut cache, conversation.state);
            let current = generation_of(&cache, record.harness);
            let reset = generation.as_deref() != Some(&current);
            let transcript = transcript(&cache, record.harness);
            let (items, previous, next) = page(
                transcript,
                if reset { None } else { before.as_deref() },
                if reset { None } else { after.as_deref() },
            )?;
            let prompt = conversation.session.and_then(|session| {
                use cide_remote::RemoteHost;
                crate::remote::AppRemoteHost::new(app.clone()).prompt(session)
            });
            Ok(ConversationResponse::Read {
                page: Box::new(ConversationPage {
                    conversation,
                    generation: current,
                    revision: transcript.revision.to_string(),
                    items,
                    before: previous,
                    after: next,
                    reset,
                    available: cache.available,
                    detail: cache.error.clone(),
                    prompt,
                }),
            })
        }
        ConversationRequest::Detail {
            project,
            id,
            generation,
            item,
            offset,
        } => {
            let record = record(app, project, &id)?;
            let cache = cache(runtime, project, &record.id);
            let mut cache = cache.lock();
            refresh(
                app,
                project,
                &record,
                &mut cache,
                summary(app, project, &record).state,
            );
            if generation != generation_of(&cache, record.harness) {
                return Err(
                    "The conversation changed; refresh it before opening these details.".into(),
                );
            }
            let entry = transcript(&cache, record.harness)
                .entries
                .iter()
                .find(|e| e.item.as_ref().is_some_and(|i| i.id == item))
                .ok_or("This message is no longer available.")?;
            let start = offset as usize;
            let text: String = entry
                .detail
                .chars()
                .skip(start)
                .take(domain::DETAIL_CHARS)
                .collect();
            let end = start + text.chars().count();
            let next = (entry.detail.chars().count() > end).then_some(end as u32);
            Ok(ConversationResponse::Detail { text, next })
        }
        action @ (ConversationRequest::Send { .. } | ConversationRequest::Resume { .. }) => {
            let (project, id, operation) = match &action {
                ConversationRequest::Send {
                    project,
                    id,
                    operation,
                    ..
                }
                | ConversationRequest::Resume {
                    project,
                    id,
                    operation,
                } => (*project, id.clone(), operation.clone()),
                _ => unreachable!(),
            };
            if operation.is_empty() || operation.len() > 128 {
                return Err("Invalid operation identifier.".into());
            }
            let fingerprint = domain::digest(
                serde_json::to_string(&action).map_err(|_| "Invalid conversation action")?,
            );
            // Claim and finish under one lock: duplicate requests cannot pass the check together.
            let mut actions = runtime.actions.lock();
            if let Some((_, previous, result)) = actions.iter().find(|(op, _, _)| op == &operation)
            {
                if previous != &fingerprint {
                    return Err(
                        "That operation identifier was already used for another action.".into(),
                    );
                }
                return result.clone();
            }
            let result = (|| {
                let record = record(app, project, &id)?;
                match action {
                    ConversationRequest::Send { session, text, .. } => {
                        send(app, runtime, project, &record, session, &operation, &text)
                    }
                    ConversationRequest::Resume { .. } => {
                        let session = resume(app, project, &record)?;
                        Ok(ConversationResponse::Receipt {
                            operation: operation.clone(),
                            status: "resumed".into(),
                            session: Some(session),
                        })
                    }
                    _ => unreachable!(),
                }
            })();
            actions.push_back((operation, fingerprint, result.clone()));
            while actions.len() > 1024 {
                actions.pop_front();
            }
            result
        }
    }
}

fn cache(runtime: &Runtime, project: ProjectId, id: &str) -> Arc<Mutex<Cache>> {
    let mut caches = runtime.caches.lock();
    if caches.len() >= 32
        && !caches.contains_key(&(project, id.into()))
        && let Some(key) = caches.keys().next().cloned()
    {
        caches.remove(&key);
    }
    caches.entry((project, id.into())).or_default().clone()
}
fn transcript(cache: &Cache, harness: Harness) -> &Transcript {
    if harness.reads_provider_document() {
        &cache.export
    } else {
        &cache.jsonl.transcript
    }
}
fn generation_of(cache: &Cache, harness: Harness) -> String {
    format!(
        "{}:{}",
        cache.seed,
        if harness.reads_provider_document() {
            0
        } else {
            cache.jsonl.generation
        }
    )
}
fn refresh(
    app: &AppHandle,
    project: ProjectId,
    record: &SessionRecord,
    cache: &mut Cache,
    state: Option<SessionState>,
) {
    let interval = if matches!(state, Some(SessionState::Busy)) {
        2
    } else {
        5
    };
    if record.harness.reads_provider_document()
        && cache.state == state
        && cache
            .checked
            .is_some_and(|t| t.elapsed() < Duration::from_secs(interval))
    {
        return;
    }
    cache.state = state;
    cache.checked = Some(Instant::now());
    let result = if record.harness.reads_provider_document() {
        export(app, project, record).map(|value| {
            if cache.export.reconcile(Transcript::export(&value)) {
                cache.seed = uuid::Uuid::new_v4().to_string();
            }
        })
    } else {
        let path = crate::sessions_state::handle(app)
            .and_then(|s| s.input_path(record))
            .or_else(|| cache.path.clone().filter(|p| p.is_file()));
        match path {
            Some(path) => {
                if cache.path.as_ref().is_some_and(|old| old != &path) { cache.jsonl = JsonlReader::default(); cache.seed = uuid::Uuid::new_v4().to_string(); }
                cache.path = Some(path.clone());
                cache.jsonl.refresh(&path).map_err(|_| "The saved conversation could not be read.".to_string())
            }
            None => Err("The harness has not saved this conversation yet, or its history is no longer available.".into()),
        }
    };
    cache.available = result.is_ok();
    cache.error = result.err();
}

fn export(
    app: &AppHandle,
    project: ProjectId,
    record: &SessionRecord,
) -> Result<serde_json::Value, String> {
    use cide_agents::harness::opencode::{Flavor, Generation};
    if !record.known
        || !record.id.starts_with("ses_")
        || !record
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("The harness has not named this conversation yet.".into());
    }
    let flavor = Flavor::of(record.harness).ok_or("No conversation exporter for this harness")?;
    let settings = app
        .state::<WorkspaceState>()
        .with(|ws| cide_core::harness_settings::effective(ws, Some(project)));
    let cli = &settings.opencode.cli;
    let binary = if record.harness == Harness::Mimo {
        flavor.program()
    } else {
        cli.binary.trim()
    };
    let generation = if record.harness == Harness::Mimo {
        flavor.cli_flags().generation
    } else {
        flavor.probe_with_cli(cli).generation
    };
    let mut command = std::process::Command::new(binary);
    if generation == Generation::V2 {
        command.arg("session");
    }
    command.arg("export").arg(&record.id);
    if generation == Generation::V2 {
        command.arg("--standalone");
    }
    command.current_dir(if record.cwd.is_dir() {
        record.cwd.clone()
    } else {
        root(app, project)?
    });
    if record.harness == Harness::Opencode {
        for variable in &cli.env {
            command.env(&variable.name, &variable.value);
        }
    }
    let output = domain::run_export(command, Duration::from_secs(10), 64 * 1024 * 1024)?;
    let value: serde_json::Value = serde_json::from_slice(&output)
        .map_err(|_| "The harness returned an unrecognized conversation export.")?;
    let root = value.get("data").unwrap_or(&value);
    if !root.is_array() && !root["messages"].is_array() {
        return Err("The harness returned an unrecognized conversation export.".into());
    }
    Ok(value)
}

fn page(
    t: &Transcript,
    before: Option<&str>,
    after: Option<&str>,
) -> Result<(Vec<ConversationItem>, Option<String>, String), String> {
    if before.is_some() && after.is_some() {
        return Err("Choose history or updates, not both.".into());
    }
    if let Some(after) = after {
        let revision: u64 = after.parse().map_err(|_| "Invalid updates cursor")?;
        if revision > t.revision {
            return Err("The updates cursor is no longer valid.".into());
        }
        let mut changed: Vec<_> = t.entries.iter().filter(|e| e.revision > revision).collect();
        changed.sort_by_key(|e| e.revision);
        changed.truncate(domain::PAGE_SIZE);
        let next = changed
            .last()
            .map_or(t.revision, |e| e.revision)
            .to_string();
        return Ok((
            changed.iter().filter_map(|e| e.item.clone()).collect(),
            None,
            next,
        ));
    }
    let end = before
        .map(|b| b.parse::<usize>().map_err(|_| "Invalid history cursor"))
        .transpose()?
        .unwrap_or(t.entries.len())
        .min(t.entries.len());
    let start = end.saturating_sub(domain::PAGE_SIZE);
    Ok((
        t.entries[start..end]
            .iter()
            .filter_map(|e| e.item.clone())
            .collect(),
        (start != 0).then(|| start.to_string()),
        t.revision.to_string(),
    ))
}

fn send(
    app: &AppHandle,
    runtime: &Runtime,
    project: ProjectId,
    record: &SessionRecord,
    session: SessionId,
    operation: &str,
    text: &str,
) -> Result<ConversationResponse, String> {
    if text.trim().is_empty()
        || text.len() > 256 * 1024
        || text
            .chars()
            .any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t'))
    {
        return Err(
            "Send a nonempty prompt of at most 256 KiB, without terminal control characters."
                .into(),
        );
    }
    if live(app, record) != Some(session) {
        return Err(
            "This console changed conversation or stopped. Refresh it before sending.".into(),
        );
    }
    if !summary(app, project, record).writable {
        return Err(
            "This is an agent-owned conversation. Open an interactive continuation to chat.".into(),
        );
    }
    let state = crate::emit::last_session_states().get(&session).copied();
    if matches!(
        state,
        Some(SessionState::AwaitingPermission | SessionState::Paused | SessionState::Exited { .. })
    ) {
        return Err(
            "Answer the pending approval or resume the session before sending a prompt.".into(),
        );
    }
    if let Some(console) = runtime.consoles.get(&session) {
        if console.conversation().id != record.id {
            return Err("The console changed conversation; refresh it before sending.".into());
        }
        console.peer_prompt(text).map_err(|_| {
            "Submission is uncertain. Check the conversation before sending again.".to_string()
        })?;
    } else {
        let registry = app.state::<SessionRegistry>();
        let pty = registry
            .get(session)
            .ok_or("This console is no longer running.")?;
        let capture = pty.capture_screen();
        if !domain::composer(record.harness, &capture).is_some_and(|text| text.trim().is_empty()) {
            return Err("The desktop input contains a draft or menu. Open Terminal to finish it before sending.".into());
        }
        // Claim the same input lane as desktop writes for the entire paste/submit transaction.
        crate::peer_chat::submit_mobile(app, session, &pty, record.harness, text)?;
    }
    crate::agent_rpc::note_typed(session);
    Ok(ConversationResponse::Receipt {
        operation: operation.into(),
        status: "submitted".into(),
        session: Some(session),
    })
}

fn resume(
    app: &AppHandle,
    project: ProjectId,
    record: &SessionRecord,
) -> Result<SessionId, String> {
    if let Some(session) = live(app, record) {
        return Ok(session);
    }
    if !resume_enabled(app, project, record.harness) {
        return Err("Session restoration is disabled in Settings → Harness for this CLI.".into());
    }
    if record.session.is_some_and(|s| {
        app.state::<Arc<crate::agents::AgentRegistry>>()
            .owns_session(s)
    }) {
        return Err("This conversation is still owned by an agent run. Stop the run before opening an interactive continuation.".into());
    }
    if !record.known {
        return Err("The harness never reported an identity for this conversation.".into());
    }
    if !record.cwd.is_dir() {
        return Err(
            "The original working directory is gone. Saved history can still be read.".into(),
        );
    }
    let conversation = HarnessSession {
        harness: record.harness,
        id: record.id.clone(),
        cwd: record.cwd.clone(),
    };
    let harness = match record.harness {
        Harness::Claude => Some(cide_ipc::ConsoleHarness::Claude),
        Harness::Codex => Some(cide_ipc::ConsoleHarness::Codex),
        Harness::Opencode => Some(cide_ipc::ConsoleHarness::Opencode),
        _ => None,
    };
    // Reuse the desktop's availability and resume-policy checks, including run ownership.
    let plan = tokio::runtime::Handle::current()
        .block_on(crate::cmd::sessions::sessions_open(
            app.clone(),
            app.state::<Arc<crate::agents::AgentRegistry>>(),
            app.state::<SessionRegistry>(),
            project,
            record.id.clone(),
        ))
        .map_err(|e| e.to_string())?;
    if let cide_ipc::RunOpen::Unavailable { reason } = plan {
        return Err(reason);
    }
    let registry = app.state::<SessionRegistry>();
    let request = crate::cmd::session::SpawnRequest {
        program: String::new(),
        harness,
        args: vec![],
        cwd: record.cwd.to_string_lossy().into_owned(),
        geometry: cide_ipc::Geometry::default(),
        project: Some(project),
        resume: None,
        resume_picker: false,
        fork: None,
        continues: Some(conversation.clone()),
        pane: None,
        voice: None,
        env: vec![],
        prompt: None,
        task_tools: true,
    };
    let session = match tokio::runtime::Handle::current()
        .block_on(crate::cmd::session::spawn_session(app, &registry, request))
    {
        Ok(session) => session,
        Err(crate::cmd::session::SessionError::AlreadyOpen(session)) => return Ok(session),
        Err(error) => return Err(error.to_string()),
    };
    registry.note_harness(session, record.harness);
    let pane = PaneId::new();
    let opened = app.state::<WorkspaceState>().update(|ws| {
        let tab = cide_core::workspace::open_tab(
            ws,
            project,
            TabKind::ClaudeFull {
                title: record.name.clone().unwrap_or_else(|| record.title.clone()),
                ephemeral: true,
            },
            Pane {
                id: pane,
                kind: PaneKind::Claude,
                role: PaneRole::Auxiliary,
                session: Some(session),
                conversation: None,
                conversation_since: None,
                codex_cleared: false,
                harness_conversation: Some(conversation.clone()),
                continues: Some(conversation.clone()),
                harness: Some(record.harness),
                title: record.title.clone(),
                docker: None,
                origin: None,
            },
        )?;
        crate::cmd::pane::bind_recorded_session(ws, &registry, project, tab, pane, session)?;
        Ok(tab)
    });
    if let Err(e) = opened {
        if let Some(pty) = registry.get(session) {
            pty.kill();
        }
        return Err(e.to_string());
    }
    crate::claude_tab::bind_ide_pane(app, &registry, project, session, pane);
    Ok(session)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restoration_uses_each_harnesses_own_resume_policy() {
        let mut settings = cide_ipc::Settings::default();
        settings.claude.cli.inject.resume.enabled = false;
        settings.codex.cli.inject.resume = true;
        settings.opencode.cli.inject.resume = false;
        assert!(!policy_allows_resume(&settings, Harness::Claude));
        assert!(policy_allows_resume(&settings, Harness::Codex));
        assert!(!policy_allows_resume(&settings, Harness::Opencode));
        assert!(policy_allows_resume(&settings, Harness::Qwen));
        assert!(policy_allows_resume(&settings, Harness::Mimo));
        settings.codex.cli.inject.resume = false;
        settings.opencode.cli.inject.resume = true;
        assert!(!policy_allows_resume(&settings, Harness::Codex));
        assert!(policy_allows_resume(&settings, Harness::Opencode));
    }

    #[test]
    fn history_pages_and_updates_do_not_skip_changes_to_old_tools() {
        let mut t = Transcript::default();
        for i in 0..75 {
            t.feed(&serde_json::json!({"type":"user","uuid":i.to_string(),"message":{"content":"same"}}), &i.to_string());
        }
        let (latest, before, after) = page(&t, None, None).unwrap();
        assert_eq!(latest.len(), 50);
        assert_eq!(before.as_deref(), Some("25"));
        assert_eq!(page(&t, before.as_deref(), None).unwrap().0.len(), 25);
        t.feed(
            &serde_json::json!({"type":"user","uuid":"0","message":{"content":"changed"}}),
            "0",
        );
        let (changed, _, _) = page(&t, None, Some(&after)).unwrap();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].ordinal, 0);
    }
}
