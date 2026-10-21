//! Runtime glue for the opt-in decision experiment. No trace is written to a file or log.
use crate::workspace_state::WorkspaceState;
use cide_core::{decision as policy, workspace};
use cide_ipc::*;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone)]
pub(crate) struct Runtime {
    inner: Arc<Mutex<Inner>>,
    permits: Arc<tokio::sync::Semaphore>,
    queue: Arc<tokio::sync::Semaphore>,
    next: Arc<AtomicU64>,
}
#[derive(Default)]
struct Inner {
    sessions: HashMap<SessionId, Session>,
    windows: HashMap<String, policy::Journal>,
    endpoint: Option<String>,
    natives: HashMap<(SessionId, String), SessionId>,
    effects: HashMap<u64, (SessionId, u64, u64, Vec<DecisionCheck>)>,
    failures: u8,
    paused_until: Option<Instant>,
}
struct Session {
    project: ProjectId,
    harness: ConsoleHarness,
    lineage: String,
    root: std::path::PathBuf,
    token: String,
    metrics: DecisionMetrics,
    published: DecisionMetrics,
    last_window: Option<String>,
    tracker: policy::Tracker,
    adapter: bool,
    pending: bool,
    requested_model: Option<String>,
    automatic_model: Option<String>,
}
impl Default for Runtime {
    fn default() -> Self {
        Self {
            inner: Arc::default(),
            permits: Arc::new(tokio::sync::Semaphore::new(2)),
            queue: Arc::new(tokio::sync::Semaphore::new(8)),
            next: Arc::new(AtomicU64::new(1)),
        }
    }
}
#[derive(Clone)]
pub(crate) struct Handle {
    app: AppHandle,
    pub session: SessionId,
}
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct Action {
    pub id: u64,
    pub generation: u64,
    pub text: String,
    pub profile: Option<DecisionProfile>,
    pub continuation: bool,
}
type Evaluation = (
    BTreeMap<String, policy::Answer>,
    Vec<u64>,
    u64,
    DecisionMode,
);
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(crate) fn configured(app: &AppHandle, project: Option<ProjectId>) -> bool {
    app.state::<WorkspaceState>().with(|ws| {
        project
            .and_then(|p| cide_core::harness_settings::overrides(ws, p).ok())
            .and_then(|p| p.decision)
            .unwrap_or_default()
            .enabled
    })
}

pub(crate) fn register(
    app: &AppHandle,
    session: SessionId,
    project: ProjectId,
    harness: ConsoleHarness,
    root: std::path::PathBuf,
    lineage: String,
    pinned: bool,
) -> Handle {
    let lineage = app.state::<WorkspaceState>().with(|ws| {
        ws.projects
            .get(&project)
            .and_then(|p| {
                p.tabs.iter().find_map(|t| {
                    t.peer_chat
                        .as_ref()
                        .filter(|pair| {
                            pair.main.to_string() == lineage || pair.peer.to_string() == lineage
                        })
                        .map(|pair| pair.main.to_string())
                })
            })
            .unwrap_or(lineage)
    });
    let runtime = app.state::<Runtime>();
    runtime
        .inner
        .lock()
        .sessions
        .entry(session)
        .or_insert_with(|| Session {
            project,
            harness,
            lineage,
            root,
            token: format!("{}{}", SessionId::new(), SessionId::new()),
            adapter: false,
            pending: false,
            requested_model: None,
            automatic_model: None,
            published: Default::default(),
            last_window: None,
            metrics: DecisionMetrics {
                session: session.to_string(),
                project: Some(project),
                harness: match harness {
                    ConsoleHarness::Codex => "codex",
                    _ => "opencode",
                }
                .into(),
                pinned,
                note: "Available on the next compatible launch".into(),
                ..Default::default()
            },
            tracker: Default::default(),
        });
    Handle {
        app: app.clone(),
        session,
    }
}
pub(crate) fn handle(app: &AppHandle, session: SessionId) -> Option<Handle> {
    app.try_state::<Runtime>()?
        .inner
        .lock()
        .sessions
        .contains_key(&session)
        .then(|| Handle {
            app: app.clone(),
            session,
        })
}
impl Runtime {
    pub fn forget_window(&self, label: &str) {
        self.inner.lock().windows.remove(label);
    }
}
impl Handle {
    pub fn enabled(&self) -> bool {
        self.config().is_some_and(|(_, project, _)| project.enabled)
    }
    fn runtime(&self) -> Runtime {
        self.app.state::<Runtime>().inner().clone()
    }
    fn config(&self) -> Option<(DecisionSettings, ProjectDecisionSettings, String)> {
        let rt = self.runtime();
        let (project, lineage) = {
            let inner = rt.inner.lock();
            let s = inner.sessions.get(&self.session)?;
            (s.project, s.lineage.clone())
        };
        self.app.state::<WorkspaceState>().with(|ws| {
            let p = cide_core::harness_settings::overrides(ws, project)
                .ok()?
                .decision
                .unwrap_or_default();
            let group = policy::group(&p, &lineage).to_string();
            Some((ws.settings.decision.clone(), p, group))
        })
    }
    fn window(&self) -> Option<String> {
        self.app.state::<WorkspaceState>().with(|ws| {
            let pane = workspace::session_panes(ws, None)
                .into_iter()
                .find(|p| p.session == self.session);
            let project = pane.as_ref().map(|p| p.project).or_else(|| {
                self.runtime()
                    .inner
                    .lock()
                    .sessions
                    .get(&self.session)
                    .map(|s| s.project)
            })?;
            if let Some(pane) = pane {
                for (label, window) in &ws.windows {
                    if matches!(window,WindowRole::DetachedPane{pane:id,..} if *id==pane.pane.id)
                        || matches!(window,WindowRole::DetachedTab{tab,..} if Some(*tab)==pane.tab)
                    {
                        return Some(label.0.clone());
                    }
                }
            }
            workspace::windows_for_project(ws, project)
                .into_iter()
                .find(|label| self.app.get_webview_window(&label.0).is_some())
                .map(|l| l.0)
        })
    }
    pub fn requested_model(&self, model: Option<&str>) {
        if let Some(model) = model
            && let Some(s) = self.runtime().inner.lock().sessions.get_mut(&self.session)
        {
            if s.requested_model.as_deref().is_some_and(|old| old != model)
                && s.automatic_model.as_deref() != Some(model)
            {
                s.metrics.pinned = true;
            }
            s.requested_model = Some(model.into());
        }
    }
    pub fn selected_model(&self, model: &str) {
        if let Some(s) = self.runtime().inner.lock().sessions.get_mut(&self.session) {
            s.automatic_model = Some(model.into());
        }
    }
    pub fn available(&self, available: bool, note: &str) {
        if let Some(s) = self.runtime().inner.lock().sessions.get_mut(&self.session) {
            s.adapter = available;
            s.metrics.available = available;
            s.metrics.note = note.into();
        }
        self.publish();
    }
    fn publish(&self) {
        let Some(window) = self.window() else { return };
        let Some((global, config, group)) = self.config() else {
            return;
        };
        let rt = self.runtime();
        let mut inner = rt.inner.lock();
        let Some(s) = inner.sessions.get_mut(&self.session) else {
            return;
        };
        s.metrics.enabled = config.enabled;
        s.metrics.group = group;
        s.metrics.period = policy::period(&global, &config);
        let metrics = s.metrics.clone();
        let previous = s.published.clone();
        s.published = metrics.clone();
        s.last_window = Some(window.clone());
        let journal = inner.windows.entry(window).or_default();
        let key = format!("{}:{}", metrics.session, metrics.period);
        let row = journal
            .sessions
            .entry(key)
            .or_insert_with(|| DecisionMetrics {
                session: metrics.session.clone(),
                project: metrics.project,
                harness: metrics.harness.clone(),
                group: metrics.group.clone(),
                period: metrics.period.clone(),
                ..Default::default()
            });
        row.enabled = metrics.enabled;
        row.pinned = metrics.pinned;
        row.available = metrics.available;
        row.note = metrics.note;
        row.turns += metrics.turns.saturating_sub(previous.turns);
        row.tools += metrics.tools.saturating_sub(previous.tools);
        row.failures += metrics.failures.saturating_sub(previous.failures);
        row.continuations += metrics.continuations.saturating_sub(previous.continuations);
        row.elapsed_ms += metrics.elapsed_ms.saturating_sub(previous.elapsed_ms);
        fn delta(target: &mut Option<u64>, current: Option<u64>, previous: Option<u64>) {
            if let Some(n) = current {
                *target = Some(target.unwrap_or(0) + n.saturating_sub(previous.unwrap_or(0)));
            }
        }
        delta(
            &mut row.input_tokens,
            metrics.input_tokens,
            previous.input_tokens,
        );
        delta(
            &mut row.output_tokens,
            metrics.output_tokens,
            previous.output_tokens,
        );
        delta(
            &mut row.cache_tokens,
            metrics.cache_tokens,
            previous.cache_tokens,
        );
        if let Some(cost) = metrics.cost {
            row.cost = Some(row.cost.unwrap_or(0.) + (cost - previous.cost.unwrap_or(0.)).max(0.));
        }
        journal.revision += 1;
    }
    pub fn current(&self, generation: u64) -> bool {
        self.config().is_some_and(|(_, p, _)| p.enabled)
            && self
                .runtime()
                .inner
                .lock()
                .sessions
                .get(&self.session)
                .is_some_and(|s| {
                    s.adapter
                        && s.tracker.generation == generation
                        && !s.tracker.stopped
                        && !s.tracker.waiting
                })
    }
    pub fn can_apply(&self, id: u64, generation: u64) -> bool {
        if crate::native_plan::get(&self.app, self.session).is_some_and(|plan| plan.pending()) {
            return false;
        }
        if !self.current(generation) {
            return false;
        }
        let Some((global, config, _)) = self.config() else {
            return false;
        };
        let period = policy::period(&global, &config);
        self.runtime()
            .inner
            .lock()
            .windows
            .values()
            .any(|j| j.permits(id, &self.session.to_string(), &period))
    }
    pub fn status(&self, id: u64, status: &str, reason: &str) {
        // Search the original window; moving a pane must not duplicate a decision.
        let rt = self.runtime();
        let mut inner = rt.inner.lock();
        if matches!(status, "applied" | "rejected" | "stale")
            && let Some((session, generation, revision, checks)) = inner.effects.remove(&id)
            && status == "applied"
            && let Some(s) = inner.sessions.get_mut(&session)
            && s.tracker.generation == generation
        {
            for check in checks {
                let command = check.command.trim().to_string();
                if !check.required && s.tracker.checked.get(&command) != Some(&revision) {
                    s.tracker.pending_checks.insert(command, check.description);
                }
            }
        }
        for journal in inner.windows.values_mut() {
            journal.status(id, status, reason);
        }
    }
    fn annotate(&self, id: u64, generation: u64, text: &str, checks: Vec<DecisionCheck>) {
        let rt = self.runtime();
        let mut inner = rt.inner.lock();
        if !checks.is_empty() {
            let revision = inner
                .sessions
                .get(&self.session)
                .map_or(0, |s| s.tracker.revision);
            inner
                .effects
                .insert(id, (self.session, generation, revision, checks));
        }
        for journal in inner.windows.values_mut() {
            journal.action(id, text);
        }
    }
    pub fn event(&self, event: policy::Event) -> bool {
        let Some((global, config, group)) = self.config() else {
            return false;
        };
        let rt = self.runtime();
        {
            let mut inner = rt.inner.lock();
            let Some(s) = inner.sessions.get_mut(&self.session) else {
                return false;
            };
            s.metrics.enabled = config.enabled;
            s.metrics.group = group;
            s.metrics.period = policy::period(&global, &config);
            let cancel = matches!(event.kind.as_str(), "prompt" | "stop");
            if !s.tracker.event(event.clone(), &mut s.metrics, now()) {
                return false;
            }
            s.tracker.refresh_required_checks(&config.checks);
            for journal in inner.windows.values_mut() {
                journal.observe(&self.session.to_string(), &event);
            }
            if cancel {
                inner
                    .effects
                    .retain(|_, (session, _, _, _)| *session != self.session);
            }
        }
        self.publish();
        true
    }
    pub async fn evaluate(
        &self,
        scenario: DecisionScenario,
        state: String,
        questions: BTreeMap<String, policy::Question>,
        expected_generation: Option<u64>,
    ) -> Result<Evaluation, String> {
        let (global, config, group) = self.config().ok_or("Decision session ended")?;
        let mode = policy::mode(&config, &group, scenario);
        if mode == DecisionMode::Off {
            return Err("Scenario disabled for this session or comparison group".into());
        }
        let rt = self.runtime();
        // A full broker must not consume a revision's evaluation or loop cooldown.
        let _queue = rt
            .queue
            .clone()
            .try_acquire_owned()
            .map_err(|_| "Decision queue is full")?;
        let (generation, project, harness, period) = {
            let mut inner = rt.inner.lock();
            if inner.paused_until.is_some_and(|t| Instant::now() < t) {
                return Err("Decision service cooling down after repeated errors".into());
            }
            let s = inner
                .sessions
                .get_mut(&self.session)
                .ok_or("Decision session ended")?;
            inference_ready(s.adapter, s.pending, &s.tracker)?;
            let period = policy::period(&global, &config);
            admit_scenario(
                &mut s.tracker,
                scenario,
                &period,
                now(),
                config.cooldown_secs,
                expected_generation,
            )?;
            s.pending = true;
            (
                s.tracker.generation,
                s.project,
                s.metrics.harness.clone(),
                period,
            )
        };
        struct Pending(Runtime, SessionId);
        impl Drop for Pending {
            fn drop(&mut self) {
                if let Some(s) = self.0.inner.lock().sessions.get_mut(&self.1) {
                    s.pending = false;
                }
            }
        }
        let _pending = Pending(rt.clone(), self.session);
        // At most eight queued/running sessions and one outstanding decision per session.
        let state = policy::bounded(&state, config.max_input_bytes.min(24576));
        let started = Instant::now();
        let response = tokio::select! {
            result=tokio::time::timeout(Duration::from_millis(global.timeout_ms),async {
                let _permit=rt.permits.acquire().await.map_err(|_|"Decision broker stopped".to_string())?;
                request(&global,&state,&questions).await
            })=>result.unwrap_or_else(|_|Err("Decision request timed out".into())),
            _=async {loop {tokio::time::sleep(Duration::from_millis(50)).await;if !self.current(generation) || self.config().is_none_or(|(g,p,_)|policy::period(&g,&p)!=period){break;}}}=>Err("Decision request cancelled".into()),
        };
        let latency = started.elapsed().as_millis() as u64;
        let still_current = self.current(generation)
            && self
                .config()
                .is_some_and(|(g, p, _)| policy::period(&g, &p) == period);
        let window = self.window();
        let mut inner = rt.inner.lock();
        let parsed = response
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|r| policy::answers(r, &questions));
        if !still_current {
        } else if parsed.is_err() {
            inner.failures = inner.failures.saturating_add(1);
            if inner.failures >= 3 {
                inner.paused_until = Some(Instant::now() + Duration::from_secs(30));
            }
        } else {
            inner.failures = 0;
            inner.paused_until = None;
        }
        let mut ids = vec![];
        for (name, q) in &questions {
            let answer = parsed.as_ref().ok().and_then(|a| a.get(name));
            let selection =
                answer.map(|a| policy::select(scenario, a, config.rule(scenario).threshold));
            let id = rt.next.fetch_add(1, Ordering::Relaxed);
            ids.push(id);
            let (status, reason) = if !still_current {
                ("stale", "The session or configuration changed".to_string())
            } else if let Err(error) = &parsed {
                ("error", error.clone())
            } else if mode == DecisionMode::Observe {
                ("observed", "Observation mode; no action applied".into())
            } else if let Some(selection) = &selection {
                (selection.status, selection.reason.clone())
            } else {
                ("proposed", "Awaiting harness acceptance".into())
            };
            if let Some(window) = &window {
                inner
                    .windows
                    .entry(window.clone())
                    .or_default()
                    .push(DecisionRecord {
                        id,
                        at_ms: now(),
                        project,
                        session: self.session.to_string(),
                        harness: harness.clone(),
                        scenario,
                        period: period.clone(),
                        group: group.clone(),
                        mode,
                        question: q.instructions.clone(),
                        state: state.clone(),
                        probabilities: answer
                            .map(|a| {
                                a.probabilities
                                    .iter()
                                    .map(|(key, probability)| {
                                        (
                                            format!(
                                                "{key} · {}",
                                                q.criteria[key].as_str().unwrap_or(key)
                                            ),
                                            *probability,
                                        )
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                        intervention: selection.as_ref().and_then(|s| s.intervention.clone()),
                        choice: selection
                            .as_ref()
                            .map(|a| {
                                q.criteria[&a.choice]
                                    .as_str()
                                    .unwrap_or(&a.choice)
                                    .to_string()
                            })
                            .unwrap_or_default(),
                        status: status.into(),
                        reason,
                        action: String::new(),
                        outcome: String::new(),
                        latency_ms: latency,
                        input_tokens: response
                            .as_ref()
                            .ok()
                            .and_then(|r| r["usage"]["input_tokens"].as_u64()),
                        feedback: None,
                    });
            }
        }
        drop(inner);
        if !still_current {
            return Err("Session or decision settings changed during inference".into());
        }
        Ok((parsed?, ids, generation, mode))
    }
    async fn choose(
        &self,
        scenario: DecisionScenario,
        state: String,
        question: policy::Question,
    ) -> Option<(policy::Selection, u64, u64)> {
        self.choose_explained(scenario, state, question, None)
            .await
            .ok()
    }
    async fn choose_explained(
        &self,
        scenario: DecisionScenario,
        state: String,
        question: policy::Question,
        expected_generation: Option<u64>,
    ) -> Result<(policy::Selection, u64, u64), String> {
        let (answers, ids, generation, mode) = self
            .evaluate(
                scenario,
                state,
                BTreeMap::from([("decision".into(), question)]),
                expected_generation,
            )
            .await?;
        let answer = answers
            .get("decision")
            .ok_or("Missing decision answer")?
            .clone();
        let (_, config, _) = self.config().ok_or("Decision session ended")?;
        if mode != DecisionMode::Automatic {
            return Err("Observation mode; decision recorded without applying it".into());
        }
        let selection = policy::select(scenario, &answer, config.rule(scenario).threshold);
        if selection.status != "proposed" {
            return Err(selection.reason);
        }
        Ok((
            selection,
            *ids.first().ok_or("Missing decision record")?,
            generation,
        ))
    }
    /// Runs at a native user-turn boundary even when no model profiles or checks
    /// are configured. Research must not depend on the agent discovering our MCP tool.
    pub async fn research(&self, text: &str) -> Option<Action> {
        if text.trim().is_empty() {
            return None;
        }
        let (answer, id, generation) = self
            .choose(
                DecisionScenario::Research,
                json!({"userRequest": text}).to_string(),
                research_question(),
            )
            .await?;
        let Some(text) = research_advice(&answer.choice) else {
            self.status(
                id,
                "unchanged",
                "No additional research needed for this request",
            );
            return None;
        };
        self.annotate(id, generation, text, vec![]);
        Some(Action {
            id,
            generation,
            text: text.into(),
            profile: None,
            continuation: false,
        })
    }
    pub async fn model(&self, text: &str) -> Option<Action> {
        let (global, config, _) = self.config()?;
        let rt = self.runtime();
        let harness = {
            let inner = rt.inner.lock();
            let s = inner.sessions.get(&self.session)?;
            if s.metrics.pinned {
                return None;
            }
            s.harness
        };
        let profiles: Vec<_> = global
            .profiles
            .iter()
            .filter(|p| p.harness == harness && config.profiles.contains(&p.id))
            .cloned()
            .collect();
        if profiles.is_empty() {
            return None;
        }
        let mut choices = vec![("keep".into(), "Keep the current model and effort".into())];
        choices.extend(profiles.iter().enumerate().map(|(i, p)| {
            (
                format!("profile{i}"),
                format!("{}: {}", p.id, p.description),
            )
        }));
        let (a,id,generation)=self.choose(DecisionScenario::Model,text.into(),policy::Question::choice("Select the least expensive permitted profile sufficient for this user request. Keep the current profile when uncertain.",choices)).await?;
        if a.choice == "keep" {
            self.status(id, "unchanged", "Kept the current profile");
            return None;
        }
        let index = a.choice.strip_prefix("profile")?.parse::<usize>().ok()?;
        let profile = profiles.get(index)?.clone();
        self.annotate(
            id,
            generation,
            &format!(
                "{}: {}{}",
                profile.id,
                profile.model,
                profile
                    .effort
                    .as_ref()
                    .map(|v| format!(" · {v}"))
                    .unwrap_or_default()
            ),
            vec![],
        );
        Some(Action {
            id,
            generation,
            text: String::new(),
            profile: Some(profile),
            continuation: false,
        })
    }
    pub async fn after_tool(&self) -> Option<Action> {
        if crate::native_plan::get(&self.app, self.session).is_some_and(|plan| plan.pending()) {
            return None;
        }
        let (global, config, _) = self.config()?;
        let rt = self.runtime();
        let period = policy::period(&global, &config);
        let (state, loop_due, verify_due, root, generation) = {
            let inner = rt.inner.lock();
            let s = inner.sessions.get(&self.session)?;
            let due = s.tracker.loop_due(now(), config.cooldown_secs);
            let verify = s.tracker.verification_due(&period);
            (
                s.tracker.state(config.max_input_bytes),
                due,
                verify,
                s.root.clone(),
                s.tracker.generation,
            )
        };
        if loop_due {
            let (a, id, generation) = self
                .choose_explained(
                    DecisionScenario::Loop,
                    state.clone(),
                    policy::loop_question(),
                    Some(generation),
                )
                .await
                .ok()?;
            let text = match a.choice.as_str() {
                "inspect" => {
                    "Three similar actions failed. Inspect the error and relevant source before another attempt."
                }
                "change" => {
                    "Repeated attempts are not making progress. Try a different search or debugging approach based on the observed error."
                }
                _ => {
                    self.status(id, "unchanged", "No loop intervention justified");
                    return None;
                }
            };
            self.annotate(id, generation, text, vec![]);
            return Some(Action {
                id,
                generation,
                text: text.into(),
                profile: None,
                continuation: false,
            });
        }
        if !verify_due {
            return None;
        }
        let mut checks = config.checks.clone();
        if config.discover_checks {
            checks.extend(policy::discover_checks(&root));
        }
        let mut checks = policy::merge_checks(checks);
        {
            let inner = rt.inner.lock();
            let t = &inner.sessions.get(&self.session)?.tracker;
            checks.retain(|c| t.checked.get(c.command.trim()) != Some(&t.revision));
        }
        if checks.is_empty() {
            return None;
        }
        let (a, id, generation) = self
            .choose_explained(
                DecisionScenario::Verification,
                state,
                verification_question(&checks),
                Some(generation),
            )
            .await
            .ok()?;
        let selected = a
            .choice
            .strip_prefix("check")
            .and_then(|i| i.parse::<usize>().ok());
        let mut checks: Vec<_> = checks
            .into_iter()
            .enumerate()
            .filter(|(i, c)| c.required || Some(*i) == selected)
            .map(|(_, c)| c)
            .collect();
        {
            // A native check can finish while the model is answering. The revision
            // guard catches edits, but a successful read-only check keeps it intact.
            let inner = rt.inner.lock();
            let t = &inner.sessions.get(&self.session)?.tracker;
            checks.retain(|c| t.checked.get(c.command.trim()) != Some(&t.revision));
        }
        if checks.is_empty() {
            self.status(
                id,
                "unchanged",
                "No additional unfinished verification selected",
            );
            return None;
        }
        let text = format!(
            "Suggested verification for the current changes (use your normal tools and permission rules):\n{}\nDo not repeat a check already successful on the same file state. Report a concrete blocker if a check cannot run.",
            checks
                .iter()
                .map(|c| format!(
                    "- {}{}",
                    c.command,
                    if c.required { " (required)" } else { "" }
                ))
                .collect::<Vec<_>>()
                .join("\n")
        );
        self.annotate(id, generation, &text, checks);
        Some(Action {
            id,
            generation,
            text,
            profile: None,
            continuation: false,
        })
    }
    pub async fn completion(&self) -> Option<Action> {
        if crate::native_plan::get(&self.app, self.session).is_some_and(|plan| plan.pending()) {
            return None;
        }
        let (_, config, _) = self.config()?;
        let rt = self.runtime();
        let (state, generation) = {
            let mut inner = rt.inner.lock();
            let t = &mut inner.sessions.get_mut(&self.session)?.tracker;
            // A project setting can change after the last tool event. Required
            // checks are obligations from configuration, not Verification ACKs.
            t.refresh_required_checks(&config.checks);
            if !t.completion_due(&config) {
                return None;
            }
            (t.state(config.max_input_bytes), t.generation)
        };
        let (a, id, generation) = self
            .choose_explained(
                DecisionScenario::Completion,
                state,
                completion_question(),
                Some(generation),
            )
            .await
            .ok()?;
        if a.choice != "continue" {
            self.status(id, "unchanged", "No actionable unfinished obligation");
            return None;
        }
        let mut inner = rt.inner.lock();
        let s = inner.sessions.get_mut(&self.session)?;
        if !s.tracker.completion_due(&config) {
            return None;
        }
        s.tracker.continuations += 1;
        s.metrics.continuations += 1;
        s.tracker.active = true;
        s.tracker.began_ms = now();
        let text = format!(
            "Before finishing, address these outstanding obligations, unless blocked or awaiting user input: {}. Use normal permission rules and do not rerun already successful checks.",
            s.tracker
                .unfinished_checks()
                .keys()
                .cloned()
                .chain(s.tracker.obligations.iter().cloned())
                .collect::<Vec<_>>()
                .join(", ")
        );
        drop(inner);
        self.annotate(id, generation, &text, vec![]);
        Some(Action {
            id,
            generation,
            text,
            profile: None,
            continuation: true,
        })
    }
    pub async fn route(&self, query: &str) -> (String, Option<String>) {
        match self
            .choose_explained(
                DecisionScenario::Research,
                query.into(),
                policy::Question::choice(
                    "Choose a source for this code research question",
                    [
                        ("text".into(), "Exact text or file search".into()),
                        (
                            "symbols".into(),
                            "Declarations, symbols and references".into(),
                        ),
                        (
                            "diagnostics".into(),
                            "Compiler and language-server diagnostics".into(),
                        ),
                        ("docs".into(), "Local project documentation".into()),
                    ],
                ),
                None,
            )
            .await
        {
            Ok((a, id, _)) => {
                self.status(id, "applied", "Selected the context retrieval source");
                (a.choice, None)
            }
            Err(reason) => ("text".into(), Some(reason)),
        }
    }
    pub async fn rank(&self, query: &str, candidates: Vec<Value>) -> (Vec<Value>, Option<String>) {
        let original = candidates.clone();
        let Some((_, config, _)) = self.config() else {
            return (original, Some("Decision session ended".into()));
        };
        if candidates.is_empty() {
            return (original, Some("No retrieved candidates to rank".into()));
        }
        let questions = candidates
            .iter()
            .take(32)
            .enumerate()
            .map(|(i, _)| {
                (
                    format!("candidate{i}"),
                    policy::Question::yes_no(&format!(
                        "Does candidate {i} directly help answer the user query?"
                    )),
                )
            })
            .collect();
        let state = json!({"query":query,"candidates":candidates}).to_string();
        let (answers, ids, _, mode) = match self
            .evaluate(DecisionScenario::Context, state, questions, None)
            .await
        {
            Ok(evaluation) => evaluation,
            Err(reason) => return (original, Some(reason)),
        };
        if mode != DecisionMode::Automatic {
            return (
                original,
                Some("Observation mode; ranking recorded without applying it".into()),
            );
        }
        let mut scored: Vec<_> = candidates
            .into_iter()
            .take(32)
            .enumerate()
            .map(|(i, v)| {
                (
                    answers
                        .get(&format!("candidate{i}"))
                        .and_then(|a| a.probabilities.get("true"))
                        .copied()
                        .unwrap_or(0.),
                    v,
                )
            })
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        if !scored
            .iter()
            .any(|(p, _)| *p >= f64::from(config.rule(DecisionScenario::Context).threshold) / 100.)
        {
            return (
                original,
                Some("Ranking recorded below the configured threshold".into()),
            );
        }
        for id in ids {
            self.status(
                id,
                "applied",
                "Ranked retrieved context; conversation and instructions unchanged",
            );
        }
        (
            scored
                .into_iter()
                .take(config.context_limit)
                .map(|(_, v)| v)
                .collect(),
            None,
        )
    }
}

/// Claim work only after the broker and session slot are available. A skipped
/// request must not consume the loop cooldown or the current revision's evaluation.
fn admit_scenario(
    tracker: &mut policy::Tracker,
    scenario: DecisionScenario,
    period: &str,
    now: u64,
    cooldown: u64,
    expected_generation: Option<u64>,
) -> Result<(), &'static str> {
    if expected_generation.is_some_and(|generation| generation != tracker.generation) {
        return Err("Session changed before decision admission");
    }
    match scenario {
        DecisionScenario::Loop => {
            if !tracker.loop_due(now, cooldown) {
                return Err("Repeated failures no longer qualify or are cooling down");
            }
            tracker.last_loop_ms = now;
        }
        DecisionScenario::Verification => {
            if !tracker.claim_verification(period) {
                return Err("This file revision and configuration have already been evaluated");
            }
        }
        _ => {}
    }
    Ok(())
}

fn inference_ready(
    adapter: bool,
    pending: bool,
    tracker: &policy::Tracker,
) -> Result<(), &'static str> {
    if !adapter {
        Err("Native decision adapter is unavailable; restart this agent session")
    } else if tracker.stopped {
        Err("Decision session is stopped or interrupted")
    } else if tracker.waiting {
        Err("Decision session is awaiting user input or approval")
    } else if pending {
        Err("Another decision request is already in progress")
    } else {
        Ok(())
    }
}

fn verification_question(checks: &[DecisionCheck]) -> policy::Question {
    let mut choices = vec![(
        "none".into(),
        "No additional optional check is justified yet".into(),
    )];
    choices.extend(
        checks
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.required)
            .map(|(i, c)| {
                (
                    format!("check{i}"),
                    format!("{}: {}", c.command, c.description),
                )
            }),
    );
    if choices.len() == 1 {
        choices.push(("required".into(), "Only the mandatory checks".into()));
    }
    policy::Question::choice(
        "Choose the most relevant verification for the changed code. Required checks are always retained.",
        choices,
    )
}

fn completion_question() -> policy::Question {
    policy::Question::choice(
        "Does the evidence show a particular requested check is still unfinished and can be completed now? Never continue when the user stopped, input/approval is needed, or there is an external blocker.",
        [
            (
                "done".into(),
                "Finished, not relevant, blocked or needs user input".into(),
            ),
            (
                "continue".into(),
                "A listed verification obligation remains actionable and unfinished".into(),
            ),
        ],
    )
}

fn research_question() -> policy::Question {
    policy::Question::choice(
        "Choose the smallest useful research step before acting on this user request. \
         Choose none for self-contained requests or when additional research is unnecessary. \
         Do not infer that files, symbols or diagnostics already exist.",
        [
            (
                "none".into(),
                "Proceed without additional project research".into(),
            ),
            (
                "text".into(),
                "Locate relevant existing code with a focused text search".into(),
            ),
            (
                "symbols".into(),
                "Inspect relevant declarations and references".into(),
            ),
            (
                "diagnostics".into(),
                "Inspect current compiler or language-server diagnostics".into(),
            ),
            (
                "docs".into(),
                "Read relevant local project documentation".into(),
            ),
        ],
    )
}

fn research_advice(choice: &str) -> Option<&'static str> {
    match choice {
        "text" => Some(
            "Start with a focused search for relevant existing code. If useful, call cide_context with a precise literal or symbol to retrieve ranked project context. Use ordinary tools when the project is empty or context is unavailable.",
        ),
        "symbols" => Some(
            "Inspect the relevant declarations and references before editing. If useful, call cide_context with a precise symbol to retrieve ranked project context. Use ordinary tools when no matching symbols are available.",
        ),
        "diagnostics" => Some(
            "Inspect current compiler or language-server diagnostics relevant to the request before making changes. If useful, call cide_context with a precise diagnostic term. Use ordinary tools if diagnostics are unavailable.",
        ),
        "docs" => Some(
            "Read the relevant local project documentation before acting. If useful, call cide_context with a precise term from the request to retrieve ranked context. Use ordinary tools if no matching documentation exists.",
        ),
        _ => None,
    }
}

async fn request(
    settings: &DecisionSettings,
    state: &str,
    questions: &BTreeMap<String, policy::Question>,
) -> Result<Value, String> {
    request_with_timeout(
        settings,
        state,
        questions,
        Duration::from_millis(settings.timeout_ms),
    )
    .await
}

async fn request_with_timeout(
    settings: &DecisionSettings,
    state: &str,
    questions: &BTreeMap<String, policy::Question>,
    timeout: Duration,
) -> Result<Value, String> {
    policy::validate_connection(settings)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(timeout.min(Duration::from_secs(5)))
        .timeout(timeout)
        .build()
        .map_err(|_| "Cannot create decision client")?;
    let mut response = client
        .post(format!(
            "{}/v1/systemone",
            settings.url.trim_end_matches('/')
        ))
        .json(&json!({"model":settings.model,"state":state,"questions":questions}))
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() && e.is_connect() {
                "Decision connection timed out; check the API URL and that Ollama is reachable"
            } else if e.is_timeout() {
                "Decision request timed out"
            } else {
                "Decision endpoint unavailable"
            }
        })?;
    if !response.status().is_success() {
        return Err(format!(
            "System One returned HTTP {}",
            response.status().as_u16()
        ));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| {
        if e.is_timeout() {
            "Decision request timed out"
        } else {
            "Cannot read decision response"
        }
    })? {
        if bytes.len() + chunk.len() > 256 * 1024 {
            return Err("Decision response exceeds 256 KiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "Invalid System One JSON response".into())
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn decision_snapshot(
    app: AppHandle,
    window: tauri::Window,
    offset: usize,
    limit: usize,
    scenario: Option<DecisionScenario>,
    project: Option<ProjectId>,
) -> DecisionSnapshot {
    app.state::<Runtime>()
        .inner
        .lock()
        .windows
        .get(window.label())
        .map(|j| j.snapshot(offset, limit, scenario, project))
        .unwrap_or_default()
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn decision_clear(app: AppHandle, window: tauri::Window) {
    app.state::<Runtime>().forget_window(window.label());
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn decision_feedback(app: AppHandle, window: tauri::Window, id: u64, helpful: bool) {
    if let Some(j) = app
        .state::<Runtime>()
        .inner
        .lock()
        .windows
        .get_mut(window.label())
    {
        j.feedback(id, helpful);
    }
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) fn decision_auto(app: AppHandle, session: SessionId, enabled: bool) {
    if let Some(s) = app
        .state::<Runtime>()
        .inner
        .lock()
        .sessions
        .get_mut(&session)
    {
        s.metrics.pinned = !enabled;
        s.tracker.generation += 1;
    }
    if let Some(h) = handle(&app, session) {
        h.publish();
    }
}
#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn decision_probe(settings: DecisionSettings) -> Result<String, String> {
    // The runtime budget protects the coding turn, but is too short for a cold model.
    // This explicit test allows loading, then measures a request with the actual budget.
    tokio::time::timeout(Duration::from_secs(120), probe(&settings)).await
        .map_err(|_| "Connection test timed out after 120 s. Ollama may still be loading the model; check its logs and available memory.".to_string())?
}

async fn probe(settings: &DecisionSettings) -> Result<String, String> {
    let q = BTreeMap::from([(
        "probe".into(),
        policy::Question::yes_no("Does the state mention a test passing?"),
    )]);
    let t = Instant::now();
    let response = request_with_timeout(settings, "The unit test passed.", &q, Duration::from_secs(120)).await
        .map_err(|error| if error == "Decision request timed out" {
            "Connection test timed out after 120 s. Ollama may still be loading the model; check its logs and available memory.".to_string()
        } else { error })?;
    policy::answers(&response, &q)?;
    let initial_ms = t.elapsed().as_millis();
    let t = Instant::now();
    match request(settings, "The unit test passed.", &q).await {
        Ok(response) => {
            policy::answers(&response, &q)?;
            Ok(format!(
                "System One ready · initial request {initial_ms} ms (includes model loading, if needed) · warm probe {} ms · runtime budget {} ms",
                t.elapsed().as_millis(),
                settings.timeout_ms
            ))
        }
        Err(error) if error == "Decision request timed out" => Ok(format!(
            "System One responded · initial request {initial_ms} ms · warm probe exceeded the {} ms runtime budget. Increase Timeout (ms) if needed; larger decision inputs may take longer.",
            settings.timeout_ms
        )),
        Err(error) => Err(format!(
            "Initial request succeeded in {initial_ms} ms, but the warm probe failed: {error}"
        )),
    }
}

pub(crate) fn codex_event(frame: &Value) -> Option<policy::Event> {
    let method = frame["method"].as_str()?;
    let p = &frame["params"];
    let mut e = policy::Event::default();
    match method {
        "item/completed" => {
            let i = &p["item"];
            let kind = i["type"].as_str()?;
            if kind == "agentMessage" {
                return Some(policy::Event {
                    kind: "message".into(),
                    id: i["id"].as_str().unwrap_or_default().into(),
                    text: policy::bounded(i["text"].as_str().unwrap_or_default(), 4096),
                    ..Default::default()
                });
            }
            if !matches!(
                kind,
                "commandExecution" | "fileChange" | "mcpToolCall" | "dynamicToolCall"
            ) {
                return None;
            }
            e.kind = "tool".into();
            e.id = i["id"].as_str().unwrap_or_default().into();
            e.tool = kind.into();
            e.command = i["command"].as_str().unwrap_or_default().into();
            e.failed = i["status"] == "failed"
                || i["exitCode"].as_i64().is_some_and(|n| n != 0)
                || i.get("error").is_some_and(|v| !v.is_null());
            e.changed = kind == "fileChange" && !e.failed;
            e.text = policy::bounded(&i.to_string(), 4096);
        }
        "turn/plan/updated" => {
            e.kind = "plan".into();
            e.obligations = Some(
                p["plan"]
                    .as_array()?
                    .iter()
                    .filter(|v| v["status"] != "completed")
                    .filter_map(|v| v["step"].as_str().map(str::to_owned))
                    .collect(),
            );
        }
        "turn/completed" => {
            e.kind = "complete".into();
            e.id = p["turn"]["id"].as_str().unwrap_or_default().into();
            e.blocked = p["turn"]["status"] != "completed";
            e.text = policy::bounded(&p["turn"].to_string(), 2048);
        }
        "item/commandExecution/requestApproval"
        | "item/fileChange/requestApproval"
        | "item/tool/requestUserInput" => e.kind = "waiting".into(),
        "thread/tokenUsage/updated" => {
            e.kind = "usage".into();
            let t = &p["tokenUsage"]["total"];
            e.input_tokens = t["inputTokens"].as_u64();
            e.output_tokens = t["outputTokens"].as_u64();
            e.cache_tokens = t["cachedInputTokens"].as_u64();
        }
        _ => return None,
    }
    Some(e)
}

impl Handle {
    /// A single loopback broker, authenticated per launched session. Plugin source is a
    /// shipped asset; settings, prompts, answers and history are never embedded in it.
    pub fn install_opencode(
        &self,
        spec: &mut cide_pty::SpawnSpec,
        document: &mut Value,
        v2: bool,
    ) -> Result<(), String> {
        if !v2 {
            self.available(false, "OpenCode 1 is unsupported; use OpenCode 2");
            return Ok(());
        }
        let rt = self.runtime();
        let endpoint = {
            let mut inner = rt.inner.lock();
            if inner.endpoint.is_none() {
                let listener =
                    std::net::TcpListener::bind(("127.0.0.1", 0)).map_err(|e| e.to_string())?;
                listener.set_nonblocking(true).map_err(|e| e.to_string())?;
                let endpoint = format!(
                    "http://{}/decision",
                    listener.local_addr().map_err(|e| e.to_string())?
                );
                inner.endpoint = Some(endpoint);
                let app = self.app.clone();
                tauri::async_runtime::spawn(async move {
                    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                        return;
                    };
                    let connections = Arc::new(tokio::sync::Semaphore::new(8));
                    while let Ok((socket, _)) = listener.accept().await {
                        let Ok(permit) = connections.clone().try_acquire_owned() else {
                            continue;
                        };
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let _permit = permit;
                            let _ =
                                tokio::time::timeout(Duration::from_secs(35), bridge(app, socket))
                                    .await;
                        });
                    }
                });
            }
            inner.endpoint.clone().unwrap_or_default()
        };
        let dir = cide_core::persist::config_dir().join("decision-plugin");
        cide_core::persist::write_atomic_with_mode(
            &dir.join("index.mjs"),
            include_bytes!("decision-opencode.mjs"),
            0o600,
        )
        .map_err(|e| e.to_string())?;
        cide_core::persist::write_atomic_with_mode(
            &dir.join("package.json"),
            br#"{"name":"cide-decision","type":"module","main":"index.mjs"}"#,
            0o600,
        )
        .map_err(|e| e.to_string())?;
        let token = rt
            .inner
            .lock()
            .sessions
            .get(&self.session)
            .map(|s| s.token.clone())
            .ok_or("Decision session ended")?;
        spec.env.push(("CIDE_DECISION_ENDPOINT".into(), endpoint));
        spec.env.push(("CIDE_DECISION_TOKEN".into(), token));
        if !document["plugins"].is_array() {
            document["plugins"] = json!([]);
        }
        document["plugins"].as_array_mut().unwrap().push(json!(dir));
        self.available(
            false,
            "OpenCode 2 plugin prepared; waiting for native events",
        );
        Ok(())
    }
}
async fn bridge(app: AppHandle, mut socket: tokio::net::TcpStream) -> Result<(), String> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end;
    loop {
        let n = socket.read(&mut chunk).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(());
        }
        bytes.extend_from_slice(&chunk[..n]);
        if let Some(i) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
            header_end = i + 4;
            break;
        }
        if bytes.len() > 8192 {
            return Err("Oversized header".into());
        }
    }
    let header = std::str::from_utf8(&bytes[..header_end]).map_err(|_| "Invalid header")?;
    let fields: HashMap<_, _> = header
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.to_ascii_lowercase(), v.trim()))
        .collect();
    if !header.starts_with("POST /decision HTTP/1.1\r\n")
        || fields.contains_key("origin")
        || fields.contains_key("transfer-encoding")
    {
        return Err("Invalid request".into());
    }
    let length = fields
        .get("content-length")
        .and_then(|n| n.parse::<usize>().ok())
        .filter(|n| *n <= 65536)
        .ok_or("Invalid request length")?;
    let token = fields
        .get("authorization")
        .and_then(|s| s.strip_prefix("Bearer "))
        .ok_or("Missing token")?;
    let session = app
        .state::<Runtime>()
        .inner
        .lock()
        .sessions
        .iter()
        .find(|(_, s)| s.token == token)
        .map(|(id, _)| *id)
        .ok_or("Invalid token")?;
    while bytes.len() < header_end + length {
        let n = socket.read(&mut chunk).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(());
        }
        bytes.extend_from_slice(&chunk[..n]);
        if bytes.len() > header_end + 65536 {
            return Err("Oversized body".into());
        }
    }
    let input: Value = serde_json::from_slice(&bytes[header_end..header_end + length])
        .map_err(|_| "Invalid JSON")?;
    let session = if let Some(native) = input["native"].as_str().filter(|s| s.len() <= 256) {
        let rt = app.state::<Runtime>();
        let existing = rt
            .inner
            .lock()
            .natives
            .get(&(session, native.to_owned()))
            .copied();
        if let Some(id) = existing {
            id
        } else {
            if rt
                .inner
                .lock()
                .natives
                .keys()
                .filter(|(id, _)| *id == session)
                .count()
                >= 256
            {
                return Err("Decision child-session limit reached".into());
            }
            let data = {
                let inner = rt.inner.lock();
                let parent = inner.sessions.get(&session).ok_or("Session ended")?;
                let first = !inner.natives.keys().any(|(id, _)| *id == session);
                (
                    first,
                    parent.project,
                    parent.harness,
                    parent.root.clone(),
                    parent.lineage.clone(),
                    parent.metrics.pinned,
                )
            };
            let id = if data.0 { session } else { SessionId::new() };
            if !data.0 {
                register(&app, id, data.1, data.2, data.3, data.4, data.5)
                    .available(true, "OpenCode child session");
            }
            rt.inner
                .lock()
                .natives
                .insert((session, native.to_owned()), id);
            id
        }
    } else {
        session
    };
    let Some(h) = handle(&app, session) else {
        return Ok(());
    };
    let output = match input["op"].as_str() {
        Some("pin") => {
            if let Some(s) = h.runtime().inner.lock().sessions.get_mut(&session) {
                s.metrics.pinned = true;
                s.tracker.generation += 1;
            }
            h.publish();
            json!({})
        }
        Some("valid") => {
            json!({"valid":input["id"].as_u64().zip(input["generation"].as_u64()).is_some_and(|(id,g)|h.can_apply(id,g))})
        }
        Some("ack") => {
            if let Some(id) = input["id"].as_u64() {
                h.status(
                    id,
                    if input["ok"] == true {
                        "applied"
                    } else {
                        "rejected"
                    },
                    "OpenCode native adapter acknowledgement",
                );
            }
            json!({})
        }
        Some("event") => {
            let event: policy::Event =
                serde_json::from_value(input["event"].clone()).map_err(|_| "Invalid event")?;
            let kind = event.kind.clone();
            let text = event.text.clone();
            h.available(true, "OpenCode 2 native plugin connected");
            let mut research = None;
            let action = if h.event(event) {
                match kind.as_str() {
                    "prompt" => {
                        research = h.research(&text).await;
                        h.model(&text).await
                    }
                    "tool" => h.after_tool().await,
                    "complete" => h.completion().await,
                    _ => None,
                }
            } else {
                None
            };
            let mut result = action.map(|a| json!(a)).unwrap_or(json!({}));
            if let Some(research) = research {
                result["research"] = json!(research);
            }
            result
        }
        _ => json!({}),
    };
    let body = output.to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    socket
        .write_all(response.as_bytes())
        .await
        .map_err(|e| e.to_string())
}

impl Handle {
    pub async fn context(&self, arguments: Value) -> Result<Value, String> {
        let query = arguments["query"]
            .as_str()
            .filter(|q| !q.trim().is_empty() && q.len() <= 2048)
            .ok_or("query must be a nonempty literal or symbol, at most 2048 bytes")?;
        let rt = self.runtime();
        let (project, root) = {
            let inner = rt.inner.lock();
            let s = inner.sessions.get(&self.session).ok_or("Session ended")?;
            (s.project, s.root.clone())
        };
        let indexed_root = self.app.state::<WorkspaceState>().with(|ws| {
            workspace::project(ws, project)
                .ok()
                .and_then(|p| p.roots.first())
                .map(|r| r.path.clone())
        });
        let same_root = indexed_root.as_ref().is_some_and(|p| p == &root);
        let (route, route_note) = self.route(query).await;
        let mut candidates = Vec::new();
        let mut note = String::new();
        if !same_root && matches!(route.as_str(), "symbols" | "diagnostics") {
            note =
                "The open index belongs to a different checkout; using this worktree’s text search"
                    .into();
        }
        if route == "symbols" && same_root {
            match crate::cmd::symbols::symbol_query(
                self.app.state::<crate::files::FsRegistry>(),
                project,
                query.into(),
                Some(32),
            )
            .await
            {
                Ok(frame) => {
                    candidates = frame.items.into_iter().map(|v| json!(v)).collect();
                    if frame.running {
                        note = "Symbol index is still being built".into();
                    }
                }
                Err(_) => note = "Symbol index unavailable; using text search".into(),
            }
        } else if route == "diagnostics" && same_root {
            if let Some(d) = self
                .app
                .state::<crate::lsp::DiagnosticsRegistry>()
                .get(project)
            {
                let settings = self
                    .app
                    .state::<WorkspaceState>()
                    .with(|ws| ws.settings.inspections.clone());
                let snapshot = serde_json::to_value(d.snapshot(&settings)).unwrap_or_default();
                if let Some(items) = snapshot["items"].as_array() {
                    candidates = items
                        .iter()
                        .filter(|v| v.to_string().to_lowercase().contains(&query.to_lowercase()))
                        .take(32)
                        .cloned()
                        .collect();
                }
            }
            if candidates.is_empty() {
                note = "No matching current diagnostics; using text search".into();
            }
        }
        if candidates.is_empty() {
            let query = query.to_owned();
            let docs = route == "docs";
            candidates = tauri::async_runtime::spawn_blocking(move || {
                use cide_search::content::{Limits, Search, SearchRoot};
                let q = SearchQuery {
                    pattern: query,
                    mode: SearchMode::Literal,
                    case_sensitive: false,
                    whole_word: false,
                    scope: String::new(),
                    include: String::new(),
                };
                let regex = cide_search::content::compile(&q).map_err(|e| e.to_string())?;
                let include = cide_search::content::compile_globs(if docs {
                    "*.md,*.mdx,*.rst,*.txt"
                } else {
                    ""
                })?;
                let roots = [SearchRoot::new(root)];
                let cancel = std::sync::atomic::AtomicBool::new(false);
                let search = Search {
                    roots: &roots,
                    regex: &regex,
                    limits: Limits {
                        max_hits: 32,
                        max_hits_per_file: 4,
                        max_line_bytes: 512,
                        threads: 1,
                        ..Default::default()
                    },
                    admits: None,
                    include: include.as_ref(),
                    cancel: &cancel,
                    progress: None,
                };
                let mut out = Vec::new();
                search.run(&mut |hits| {
                    out.extend(hits.iter().map(|h| json!(h)));
                });
                Ok::<_, String>(out)
            })
            .await
            .map_err(|e| e.to_string())??;
        }
        candidates.truncate(32);
        let (candidates, rank_note) = self.rank(query, candidates).await;
        for (scenario, reason) in [
            ("Research routing", route_note),
            ("Context ranking", rank_note),
        ] {
            if let Some(reason) = reason {
                if !note.is_empty() {
                    note.push('\n');
                }
                note.push_str(&format!("{scenario}: {reason}"));
            }
        }
        Ok(json!({"source":route,"note":note,"candidates":candidates,"bounded":true}))
    }
}

/// Passive metrics reuse the normal event stream. Enabling telemetry does not replace the
/// launcher or install a plugin in control projects.
pub(crate) fn baseline_hook(app: &AppHandle, frame: &cide_claude::hook::HookFrame) {
    let Some(id) = frame.owner().and_then(|s| s.parse().ok()) else {
        return;
    };
    let Some(h) = handle(app, id) else { return };
    if h.runtime()
        .inner
        .lock()
        .sessions
        .get(&id)
        .is_some_and(|s| s.adapter)
    {
        return;
    }
    let p = &frame.payload;
    let mut e = policy::Event::default();
    match frame.event.as_str() {
        "UserPromptSubmit" => {
            // Status may have announced Busy before the native prompt hook. It already
            // opened the observed turn; enrich that event rather than counting it twice.
            if let Some(s) = h.runtime().inner.lock().sessions.get_mut(&id)
                && s.tracker.active
                && s.tracker.prompt.is_empty()
            {
                s.tracker.prompt = policy::bounded(p["prompt"].as_str().unwrap_or_default(), 8192);
                return;
            }
            e.kind = "prompt".into();
            e.text = p["prompt"].as_str().unwrap_or_default().into();
        }
        "PostToolUse" | "PostToolUseFailure" => {
            e.kind = "tool".into();
            e.id = p["tool_use_id"].as_str().unwrap_or_default().into();
            e.tool = p["tool_name"].as_str().unwrap_or_default().into();
            e.failed = frame.event == "PostToolUseFailure";
            e.command = p["tool_input"]["command"]
                .as_str()
                .unwrap_or_default()
                .into();
            e.text = policy::bounded(&p["tool_input"].to_string(), 2048);
        }
        "Stop" => e.kind = "complete".into(),
        "SessionEnd" => e.kind = "stop".into(),
        _ => return,
    }
    h.event(e);
}
pub(crate) fn baseline_state(app: &AppHandle, id: SessionId, state: SessionState) {
    let Some(h) = handle(app, id) else { return };
    let rt = h.runtime();
    let inner = rt.inner.lock();
    let Some(s) = inner.sessions.get(&id) else {
        return;
    };
    let event = baseline_event(s.adapter, s.tracker.active, state);
    drop(inner);
    if let Some(kind) = event {
        h.event(policy::Event {
            kind: kind.into(),
            ..Default::default()
        });
    }
}

fn baseline_event(adapter: bool, active: bool, state: SessionState) -> Option<&'static str> {
    // Owned adapters observe both sides of native permission requests. Their
    // asynchronous UI status hooks can arrive after the reply and must not latch
    // the tracker back into waiting after the native stream has resumed it.
    if adapter {
        return match state {
            SessionState::Exited { .. } | SessionState::Paused => Some("stop"),
            _ => None,
        };
    }
    Some(match state {
        SessionState::Busy if !active => "prompt",
        SessionState::Idle | SessionState::AwaitingInput if active => "complete",
        SessionState::AwaitingPermission => "waiting",
        SessionState::Paused | SessionState::Exited { .. } => "stop",
        _ => return None,
    })
}
pub(crate) fn usage(app: &AppHandle, id: SessionId, status: &Value) {
    let Some(h) = handle(app, id) else { return };
    // Owned adapters have a single authoritative usage stream. Mixing a plugin's
    // per-launch totals with status-line lifetime totals would count the same tokens twice.
    if h.runtime()
        .inner
        .lock()
        .sessions
        .get(&id)
        .is_some_and(|s| s.adapter)
    {
        return;
    }
    let tokens = &status["context_window"];
    h.event(policy::Event {
        kind: "usage".into(),
        input_tokens: tokens["total_input_tokens"].as_u64(),
        output_tokens: tokens["total_output_tokens"].as_u64(),
        cache_tokens: tokens["cache_read_input_tokens"].as_u64(),
        cost: status["cost"]["total_cost_usd"].as_f64(),
        ..Default::default()
    });
}

pub(crate) fn lineage(app: &AppHandle, session: SessionId) -> Option<String> {
    app.try_state::<Runtime>()?
        .inner
        .lock()
        .sessions
        .get(&session)
        .map(|s| s.lineage.clone())
}

pub(crate) fn user_input(app: &AppHandle, id: SessionId, data: &str) {
    let Some(h) = handle(app, id) else { return };
    let idle = h
        .runtime()
        .inner
        .lock()
        .sessions
        .get(&id)
        .is_some_and(|s| !s.tracker.active);
    if data.contains('\u{3}') || data == "\u{1b}" || (idle && !data.is_empty()) {
        h.event(policy::Event {
            kind: "stop".into(),
            ..Default::default()
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_admission_does_not_consume_verification_or_loop_cooldown() {
        let mut tracker = policy::Tracker::default();
        tracker.active = true;
        tracker.revision = 1;
        tracker.generation = 2;
        assert!(
            admit_scenario(
                &mut tracker,
                DecisionScenario::Verification,
                "p",
                31_000,
                30,
                Some(1)
            )
            .is_err()
        );
        assert!(tracker.last_verification.is_none());
        assert!(
            admit_scenario(
                &mut tracker,
                DecisionScenario::Verification,
                "p",
                31_000,
                30,
                Some(2)
            )
            .is_ok()
        );
        assert!(
            admit_scenario(
                &mut tracker,
                DecisionScenario::Verification,
                "p",
                31_001,
                30,
                Some(2)
            )
            .is_err()
        );
        let mut metrics = DecisionMetrics::default();
        for _ in 0..3 {
            tracker.event(
                policy::Event {
                    kind: "tool".into(),
                    tool: "shell".into(),
                    command: "node scripts/check.js".into(),
                    failed: true,
                    ..Default::default()
                },
                &mut metrics,
                32_000,
            );
        }
        assert!(
            admit_scenario(
                &mut tracker,
                DecisionScenario::Loop,
                "p",
                32_000,
                30,
                Some(1)
            )
            .is_err()
        );
        assert_eq!(tracker.last_loop_ms, 0);
        assert!(
            admit_scenario(
                &mut tracker,
                DecisionScenario::Loop,
                "p",
                32_000,
                30,
                Some(2)
            )
            .is_ok()
        );
        assert_eq!(tracker.last_loop_ms, 32_000);
        assert!(
            admit_scenario(
                &mut tracker,
                DecisionScenario::Loop,
                "p",
                32_001,
                30,
                Some(2)
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn split_intervention_uses_one_typed_request() {
        let (url, task) = fake(vec![(
            r#"{"answers":{"decision":{"type":"choice","choice":"inspect","probabilities":{"inspect":0.38,"change":0.32,"keep":0.30}}}}"#,
            Duration::ZERO,
        )]).await;
        let settings = DecisionSettings {
            url,
            ..Default::default()
        };
        let questions = BTreeMap::from([("decision".into(), policy::loop_question())]);
        let response = request(&settings, "Three MODULE_NOT_FOUND errors", &questions)
            .await
            .unwrap();
        let answers = policy::answers(&response, &questions).unwrap();
        let selected = policy::select(DecisionScenario::Loop, &answers["decision"], 50);
        assert_eq!(selected.status, "proposed");
        assert_eq!(selected.choice, "inspect");
        assert!(selected.intervention.unwrap().passed);
        let sent = task.await.unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["questions"].as_object().unwrap().len(), 1);
        assert_eq!(sent[0]["questions"]["decision"]["type"], "choice");
    }

    #[tokio::test]
    async fn p4_required_check_survives_below_threshold_verification_and_reaches_completion() {
        let (url, task) = fake(vec![
            (r#"{"answers":{"decision":{"type":"choice","choice":"none","probabilities":{"none":0.55,"required":0.45}}}}"#, Duration::ZERO),
            (r#"{"answers":{"decision":{"type":"choice","choice":"continue","probabilities":{"continue":0.97,"done":0.03}}}}"#, Duration::ZERO),
        ]).await;
        let settings = DecisionSettings {
            url,
            ..Default::default()
        };
        let config = ProjectDecisionSettings {
            enabled: true,
            checks: vec![DecisionCheck {
                command: "npm run check".into(),
                description: String::new(),
                required: true,
            }],
            ..Default::default()
        };
        let mut tracker = policy::Tracker::default();
        let mut metrics = DecisionMetrics::default();
        tracker.event(
            policy::Event {
                kind: "prompt".into(),
                text: "Change the title; leave npm run check pending for automatic follow-up."
                    .into(),
                ..Default::default()
            },
            &mut metrics,
            1,
        );
        let edit = codex_event(&json!({"method":"item/completed","params":{"item":{"id":"edit","type":"fileChange","status":"completed"}}})).unwrap();
        tracker.event(edit, &mut metrics, 2);
        tracker.refresh_required_checks(&config.checks);
        assert!(tracker.claim_verification("p4"));
        let checks =
            policy::merge_checks(config.checks.clone().into_iter().chain([DecisionCheck {
                command: "npm run check".into(),
                description: "package.json script check".into(),
                required: false,
            }]));
        let questions = BTreeMap::from([("decision".into(), verification_question(&checks))]);
        assert!(questions["decision"].criteria.get("check0").is_none());
        let reply = request(
            &settings,
            &tracker.state(config.max_input_bytes),
            &questions,
        )
        .await
        .unwrap();
        let answer = &policy::answers(&reply, &questions).unwrap()["decision"];
        assert_eq!(
            policy::select(DecisionScenario::Verification, answer, 80).status,
            "belowThreshold"
        );
        // No Verification action/ACK, no update_plan, only the separate sleep.
        for frame in [
            json!({"method":"item/completed","params":{"item":{"id":"sleep","type":"commandExecution","command":"sleep 3","exitCode":0,"status":"completed"}}}),
            json!({"method":"item/completed","params":{"item":{"id":"report","type":"agentMessage","text":"The edit is done. npm run check remains pending and can run immediately."}}}),
            json!({"method":"turn/completed","params":{"turn":{"id":"initial","status":"completed"}}}),
        ] {
            tracker.event(codex_event(&frame).unwrap(), &mut metrics, 3);
            tracker.refresh_required_checks(&config.checks);
        }
        assert!(tracker.pending_checks.is_empty());
        assert!(tracker.obligations.is_empty());
        assert!(tracker.completion_due(&config));
        let questions = BTreeMap::from([("decision".into(), completion_question())]);
        let reply = request(
            &settings,
            &tracker.state(config.max_input_bytes),
            &questions,
        )
        .await
        .unwrap();
        let answer = &policy::answers(&reply, &questions).unwrap()["decision"];
        let selection = policy::select(DecisionScenario::Completion, answer, 90);
        assert_eq!(selection.status, "proposed");
        assert_eq!(selection.choice, "continue");
        let requests = task.await.unwrap();
        assert_eq!(requests.len(), 2);
        let completion_state: Value =
            serde_json::from_str(requests[1]["state"].as_str().unwrap()).unwrap();
        assert_eq!(completion_state["requiredChecks"], json!(["npm run check"]));
        assert!(
            completion_state["unverified"]
                .get("npm run check")
                .is_some()
        );
        let checked = codex_event(&json!({"method":"item/completed","params":{"item":{"id":"check","type":"commandExecution","command":"npm run check","exitCode":0,"status":"completed"}}})).unwrap();
        tracker.event(checked, &mut metrics, 4);
        tracker.refresh_required_checks(&config.checks);
        assert!(!tracker.completion_due(&config));
    }

    #[test]
    fn context_inference_reports_why_it_cannot_run() {
        let mut tracker = policy::Tracker::default();
        let mut metrics = DecisionMetrics::default();
        tracker.event(
            policy::Event {
                kind: "prompt".into(),
                ..Default::default()
            },
            &mut metrics,
            1,
        );
        assert_eq!(inference_ready(true, false, &tracker), Ok(()));
        assert!(
            inference_ready(false, false, &tracker)
                .unwrap_err()
                .contains("adapter")
        );
        assert!(
            inference_ready(true, true, &tracker)
                .unwrap_err()
                .contains("in progress")
        );
        tracker.event(
            policy::Event {
                kind: "waiting".into(),
                ..Default::default()
            },
            &mut metrics,
            2,
        );
        assert!(
            inference_ready(true, false, &tracker)
                .unwrap_err()
                .contains("approval")
        );
        tracker.event(
            policy::Event {
                kind: "resumed".into(),
                ..Default::default()
            },
            &mut metrics,
            3,
        );
        assert_eq!(inference_ready(true, false, &tracker), Ok(()));
        tracker.event(
            policy::Event {
                kind: "stop".into(),
                ..Default::default()
            },
            &mut metrics,
            4,
        );
        assert!(
            inference_ready(true, false, &tracker)
                .unwrap_err()
                .contains("interrupted")
        );
    }

    #[test]
    fn delayed_ui_permission_status_cannot_reblock_a_native_adapter() {
        assert_eq!(
            baseline_event(true, true, SessionState::AwaitingPermission),
            None
        );
        assert_eq!(baseline_event(true, true, SessionState::Busy), None);
        assert_eq!(
            baseline_event(true, true, SessionState::AwaitingInput),
            None
        );
        assert_eq!(
            baseline_event(true, true, SessionState::Paused),
            Some("stop")
        );
        assert_eq!(
            baseline_event(false, true, SessionState::AwaitingPermission),
            Some("waiting")
        );
        assert_eq!(
            baseline_event(false, false, SessionState::Busy),
            Some("prompt")
        );
    }

    async fn fake(
        responses: Vec<(&str, Duration)>,
    ) -> (String, tokio::task::JoinHandle<Vec<Value>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let responses: Vec<_> = responses
            .into_iter()
            .map(|(body, delay)| (body.to_owned(), delay))
            .collect();
        let task = tokio::spawn(async move {
            let mut requests = vec![];
            for (body, delay) in responses {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut bytes = vec![];
                let mut buf = [0; 4096];
                loop {
                    let n = s.read(&mut buf).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&buf[..n]);
                    if let Some(at) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&bytes[..at]);
                        assert!(head.starts_with("POST /v1/systemone HTTP/1.1"));
                        let length = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|s| s.trim().parse::<usize>().ok())
                            })
                            .unwrap();
                        if bytes.len() >= at + 4 + length {
                            requests.push(
                                serde_json::from_slice(&bytes[at + 4..at + 4 + length]).unwrap(),
                            );
                            tokio::time::sleep(delay).await;
                            let _ = s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await;
                            break;
                        }
                    }
                }
            }
            requests
        });
        (url, task)
    }
    #[tokio::test]
    async fn systemone_transport_is_typed_and_bounded_by_timeout() {
        let (url, task) = fake(vec![(
            r#"{"answers":{"q":{"type":"noul","noul":0.97}},"usage":{"input_tokens":7}}"#,
            Duration::ZERO,
        )])
        .await;
        let settings = DecisionSettings {
            url,
            ..Default::default()
        };
        let q = BTreeMap::from([("q".into(), policy::Question::yes_no("A check passed?"))]);
        let result = request(&settings, "passed", &q).await.unwrap();
        assert_eq!(policy::answers(&result, &q).unwrap()["q"].choice, "true");
        let sent = task.await.unwrap().remove(0);
        assert_eq!(sent["model"], "d1");
        assert_eq!(sent["questions"]["q"]["type"], "noul");
        assert!(sent.get("messages").is_none());
        let (url, task) = fake(vec![("{}", Duration::from_millis(500))]).await;
        let settings = DecisionSettings {
            url,
            timeout_ms: 100,
            ..Default::default()
        };
        assert_eq!(
            request(&settings, "passed", &q).await.unwrap_err(),
            "Decision request timed out"
        );
        task.abort();
    }
    #[tokio::test]
    async fn prompt_research_accepts_a_no_research_decision_without_profiles_or_checks() {
        let (url, task) = fake(vec![(
            r#"{"answers":{"decision":{"type":"choice","choice":"none","probabilities":{"none":0.96,"text":0.01,"symbols":0.01,"diagnostics":0.01,"docs":0.01}}}}"#,
            Duration::ZERO,
        )]).await;
        let settings = DecisionSettings {
            url,
            ..Default::default()
        };
        assert!(settings.profiles.is_empty());
        assert!(ProjectDecisionSettings::default().checks.is_empty());
        let questions = BTreeMap::from([("decision".into(), research_question())]);
        let result = request(
            &settings,
            "Build a self-contained one-page todo app.",
            &questions,
        )
        .await
        .unwrap();
        let answer = &policy::answers(&result, &questions).unwrap()["decision"];
        assert_eq!(answer.choice, "none");
        assert!(research_advice(&answer.choice).is_none());
        for choice in ["text", "symbols", "diagnostics", "docs"] {
            assert!(research_advice(choice).unwrap().contains("cide_context"));
        }
        assert!(research_advice("unknown").is_none());
        assert_eq!(task.await.unwrap().len(), 1);
    }
    #[test]
    fn native_codex_events_include_failure_progress_stop_and_usage() {
        let e=codex_event(&json!({"method":"item/completed","params":{"item":{"id":"cmd1","type":"commandExecution","command":"cargo test","exitCode":1,"status":"completed"}}})).unwrap();
        assert!(e.failed);
        assert!(!e.changed);
        let e=codex_event(&json!({"method":"item/completed","params":{"item":{"id":"edit1","type":"fileChange","status":"completed"}}})).unwrap();
        assert!(e.changed);
        let e=codex_event(&json!({"method":"turn/completed","params":{"turn":{"id":"turn1","status":"interrupted"}}})).unwrap();
        assert!(e.blocked);
        let e=codex_event(&json!({"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"total":{"inputTokens":123,"outputTokens":45}}}})).unwrap();
        assert_eq!(e.input_tokens, Some(123));
        assert!(codex_event(&json!({"method":"item/agentMessage/delta"})).is_none());
        let e=codex_event(&json!({"method":"item/completed","params":{"item":{"id":"answer","type":"agentMessage","text":"Blocked: I need the user to provide credentials."}}})).unwrap();
        assert_eq!(e.kind, "message");
        assert!(e.text.contains("Blocked"));
    }

    #[tokio::test]
    async fn connection_test_allows_cold_load_then_checks_the_runtime_budget() {
        let response = r#"{"answers":{"probe":{"type":"noul","noul":0.97}}}"#;
        let (url, task) = fake(vec![
            (response, Duration::from_millis(250)),
            (response, Duration::ZERO),
        ])
        .await;
        let settings = DecisionSettings {
            url,
            timeout_ms: 100,
            ..Default::default()
        };
        let result = decision_probe(settings.clone()).await.unwrap();
        assert!(result.starts_with("System One ready"), "{result}");
        assert!(result.contains("warm probe"));
        assert!(result.contains("runtime budget 100 ms"));
        assert_eq!(settings.timeout_ms, 100);
        assert_eq!(task.await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn connection_test_reports_a_slow_warm_model_as_a_budget_problem() {
        let response = r#"{"answers":{"probe":{"type":"noul","noul":0.97}}}"#;
        let (url, task) = fake(vec![
            (response, Duration::ZERO),
            (response, Duration::from_millis(250)),
        ])
        .await;
        let settings = DecisionSettings {
            url,
            timeout_ms: 100,
            ..Default::default()
        };
        let result = decision_probe(settings).await.unwrap();
        assert!(result.starts_with("System One responded"), "{result}");
        assert!(result.contains("warm probe exceeded the 100 ms runtime budget"));
        task.abort();
    }

    #[tokio::test]
    async fn connection_test_still_rejects_invalid_typed_answers() {
        let response = r#"{"answers":{"probe":{"type":"noul","noul":0.97}}}"#;
        let invalid = r#"{"answers":{"probe":{"type":"noul","noul":1.5}}}"#;
        for responses in [
            vec![(invalid, Duration::ZERO)],
            vec![(response, Duration::ZERO), (invalid, Duration::ZERO)],
        ] {
            let (url, task) = fake(responses).await;
            let result = decision_probe(DecisionSettings {
                url,
                ..Default::default()
            })
            .await;
            assert_eq!(result.unwrap_err(), "Invalid yes/no probability");
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn runtime_timeout_covers_reading_the_response_body() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 4096];
            assert!(socket.read(&mut buffer).await.unwrap() > 0);
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_secs(2)).await;
        });
        let settings = DecisionSettings {
            url,
            timeout_ms: 100,
            ..Default::default()
        };
        let questions = BTreeMap::from([("q".into(), policy::Question::yes_no("A check passed?"))]);
        assert_eq!(
            request(&settings, "passed", &questions).await.unwrap_err(),
            "Decision request timed out"
        );
        task.abort();
    }

    #[tokio::test]
    #[ignore = "requires an explicitly configured local Ollama model; may load it into memory"]
    async fn connection_test_against_explicit_local_ollama() {
        let settings = DecisionSettings {
            url: std::env::var("CIDE_DECISION_PROBE_URL")
                .expect("Set CIDE_DECISION_PROBE_URL to the local Ollama server"),
            model: std::env::var("CIDE_DECISION_PROBE_MODEL")
                .expect("Set CIDE_DECISION_PROBE_MODEL to an installed model"),
            ..Default::default()
        };
        let url = reqwest::Url::parse(&settings.url).unwrap();
        assert!(
            matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")),
            "This audit is for local models only"
        );
        let result = decision_probe(settings.clone()).await.unwrap();
        assert!(
            result.starts_with("System One ready") || result.starts_with("System One responded")
        );
        println!("{result}");
        let questions = BTreeMap::from([("decision".into(), research_question())]);
        let started = Instant::now();
        let response = request(
            &settings,
            r#"{"userRequest":"Build a simple, beautiful todo app in one HTML page."}"#,
            &questions,
        )
        .await
        .unwrap();
        let answers = policy::answers(&response, &questions).unwrap();
        println!(
            "User-turn research: {} · {} ms",
            answers["decision"].choice,
            started.elapsed().as_millis()
        );
    }
}
