//! Decision assistance is an optimisation, never an authority over permissions or user input.
//! This module owns the policy and bounded, volatile journal; transports live in the shell.
use cide_ipc::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};

pub const HISTORY_LIMIT: usize = 10_000;
pub const HISTORY_BYTES: usize = 64 * 1024 * 1024;

pub fn validate_connection(settings: &DecisionSettings) -> Result<(), String> {
    let url = url::Url::parse(&settings.url).map_err(|_| "Decision URL must be an HTTP URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Decision URL must use HTTP(S), without credentials, query or fragment".into());
    }
    if settings.model.trim().is_empty() {
        return Err("Decision model is required".into());
    }
    if !(100..=30_000).contains(&settings.timeout_ms) {
        return Err("Decision timeout must be 100–30000 ms".into());
    }
    let mut ids = std::collections::HashSet::new();
    if settings.profiles.len() > 24 {
        return Err("At most 24 decision profiles are supported".into());
    }
    for p in &settings.profiles {
        if p.id.trim().is_empty()
            || !ids.insert(&p.id)
            || p.model.trim().is_empty()
            || !matches!(p.harness, ConsoleHarness::Codex | ConsoleHarness::Opencode)
        {
            return Err(
                "Profiles need unique names, a model and Codex or OpenCode as harness".into(),
            );
        }
    }
    Ok(())
}

pub fn validate_project(settings: &ProjectDecisionSettings) -> Result<(), String> {
    if settings.max_continuations > 2 {
        return Err("At most two completion continuations are allowed".into());
    }
    if !(1024..=24576).contains(&settings.max_input_bytes)
        || !(1..=32).contains(&settings.context_limit)
        || settings.cooldown_secs < 5
        || settings.cooldown_secs > 600
    {
        return Err(
            "Decision limits: input 1024–24576 bytes, context 1–32, cooldown 5–600 seconds".into(),
        );
    }
    if settings
        .scenarios
        .values()
        .any(|r| r.threshold > 100 || r.threshold < 50)
    {
        return Err("Decision probability thresholds must be 50–100%".into());
    }
    if settings.checks.len() > 24
        || settings.profiles.len() > 24
        || settings
            .checks
            .iter()
            .any(|c| c.command.trim().is_empty() || c.command.len() > 4096)
    {
        return Err(
            "At most 24 profiles/checks; check commands must be nonempty and at most 4096 bytes"
                .into(),
        );
    }
    Ok(())
}

pub fn period(global: &DecisionSettings, project: &ProjectDecisionSettings) -> String {
    blake3::hash(&serde_json::to_vec(&(3_u8, global, project)).unwrap_or_default()).to_hex()[..12]
        .into()
}
/// The caller passes the root session/pair/run lineage, never an individual child id.
pub fn group(settings: &ProjectDecisionSettings, lineage: &str) -> &'static str {
    if !settings.enabled {
        "baseline"
    } else if settings.ab_enabled
        && blake3::hash(lineage.as_bytes()).as_bytes()[0].is_multiple_of(2)
    {
        "control"
    } else {
        "treatment"
    }
}
pub fn mode(
    settings: &ProjectDecisionSettings,
    group: &str,
    scenario: DecisionScenario,
) -> DecisionMode {
    if !settings.enabled
        || (group == "control" && settings.ab_scenario.is_none_or(|s| s == scenario))
    {
        DecisionMode::Off
    } else {
        settings.rule(scenario).mode
    }
}

pub fn bounded(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    #[serde(rename = "type")]
    pub kind: String,
    pub instructions: String,
    pub criteria: Value,
}
impl Question {
    pub fn choice(instructions: &str, choices: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            kind: "choice".into(),
            instructions: instructions.into(),
            criteria: json!(choices.into_iter().collect::<BTreeMap<_, _>>()),
        }
    }
    pub fn yes_no(instructions: &str) -> Self {
        Self {
            kind: "noul".into(),
            instructions: instructions.into(),
            criteria: json!({"false":"No", "true":"Yes"}),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Answer {
    pub choice: String,
    pub probabilities: BTreeMap<String, f64>,
}
impl Answer {
    pub fn probability(&self) -> f64 {
        self.probabilities.get(&self.choice).copied().unwrap_or(0.)
    }
}

pub fn loop_question() -> Question {
    Question::choice(
        "Three consecutive tool actions failed without changes. Which next step is justified by the errors?",
        [
            ("keep".into(), "No intervention".into()),
            ("inspect".into(), "Inspect diagnostics and the relevant source before another attempt".into()),
            ("change".into(), "Use a different search or debugging approach; do not repeat the same failing command".into()),
        ],
    )
}

pub struct Selection {
    pub choice: String,
    pub intervention: Option<DecisionIntervention>,
    pub status: &'static str,
    pub reason: String,
}

/// Gate the need for intervention independently from the preferred kind of advice.
/// Both stages reuse the same validated distribution, with no additional model call.
pub fn select(scenario: DecisionScenario, answer: &Answer, threshold: u8) -> Selection {
    let mut choice = answer.choice.clone();
    let mut probability = answer.probability();
    let mut intervention = None;
    let mut keep = false;
    if scenario == DecisionScenario::Loop {
        let inspect = answer.probabilities.get("inspect").copied().unwrap_or(0.);
        let change = answer.probabilities.get("change").copied().unwrap_or(0.);
        let none = answer.probabilities.get("keep").copied().unwrap_or(0.);
        let total = inspect + change + none;
        let needed = inspect + change;
        // The parser permits small rounding error in the total. Normalise the
        // grouped distribution and its conditional choices before thresholding.
        probability = if total > 0. { needed / total } else { 0. };
        keep = needed <= none;
        choice = if keep {
            "keep"
        } else if inspect >= change {
            "inspect"
        } else {
            "change"
        }
        .into();
        intervention = Some(DecisionIntervention {
            probability,
            threshold,
            passed: !keep && probability >= f64::from(threshold) / 100.,
            choices: if needed > 0. {
                BTreeMap::from([
                    ("Inspect diagnostics and source".into(), inspect / needed),
                    (
                        "Change the search or debugging approach".into(),
                        change / needed,
                    ),
                ])
            } else {
                BTreeMap::new()
            },
        });
    }
    let (status, reason) = if keep {
        (
            "unchanged",
            "No intervention is at least as likely as intervention".into(),
        )
    } else if probability < f64::from(threshold) / 100. {
        (
            "belowThreshold",
            if intervention.is_some() {
                format!(
                    "Combined intervention probability {:.1}% is below the {threshold}% threshold",
                    probability * 100.
                )
            } else {
                "Below configured probability threshold".into()
            },
        )
    } else {
        (
            "proposed",
            if intervention.is_some() {
                format!(
                    "Combined intervention probability {:.1}% meets the {threshold}% threshold; preferred response selected; awaiting harness acceptance",
                    probability * 100.
                )
            } else {
                "Awaiting harness acceptance".into()
            },
        )
    };
    Selection {
        choice,
        intervention,
        status,
        reason,
    }
}
/// Reject malformed/partial responses. In particular `confidence` is not a probability.
pub fn answers(
    response: &Value,
    questions: &BTreeMap<String, Question>,
) -> Result<BTreeMap<String, Answer>, String> {
    let raw = response["answers"]
        .as_object()
        .ok_or("System One response has no answers")?;
    let mut out = BTreeMap::new();
    for (name, q) in questions {
        let a = raw.get(name).ok_or("System One omitted an answer")?;
        if a["type"] != q.kind {
            return Err("System One answer type does not match its question".into());
        }
        let answer = if q.kind == "noul" {
            let p = a["noul"]
                .as_f64()
                .filter(|p| p.is_finite() && (0.0..=1.0).contains(p))
                .ok_or("Invalid yes/no probability")?;
            Answer {
                choice: if p >= 0.5 { "true" } else { "false" }.into(),
                probabilities: BTreeMap::from([("true".into(), p), ("false".into(), 1. - p)]),
            }
        } else if q.kind == "choice" {
            let criteria = q.criteria.as_object().ok_or("Invalid choice criteria")?;
            let probabilities: BTreeMap<String, f64> =
                serde_json::from_value(a["probabilities"].clone())
                    .map_err(|_| "Invalid choice probabilities")?;
            if probabilities.len() != criteria.len()
                || probabilities.iter().any(|(k, p)| {
                    !criteria.contains_key(k) || !p.is_finite() || !(0.0..=1.0).contains(p)
                })
                || (probabilities.values().sum::<f64>() - 1.).abs() > 0.02
            {
                return Err("Invalid choice probability distribution".into());
            }
            let choice = a["choice"]
                .as_str()
                .filter(|c| criteria.contains_key(*c))
                .ok_or("Unknown decision choice")?;
            if probabilities
                .values()
                .any(|p| *p > probabilities[choice] + 0.00001)
            {
                return Err("Decision choice is not its most probable option".into());
            }
            Answer {
                choice: choice.into(),
                probabilities,
            }
        } else {
            return Err("Unsupported decision question type".into());
        };
        out.insert(name.clone(), answer);
    }
    Ok(out)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Event {
    pub kind: String,
    pub id: String,
    pub text: String,
    pub tool: String,
    pub command: String,
    pub failed: bool,
    pub changed: bool,
    pub blocked: bool,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_tokens: Option<u64>,
    pub cost: Option<f64>,
    pub obligations: Option<Vec<String>>,
}

#[derive(Debug, Default)]
pub struct Tracker {
    pub generation: u64,
    pub prompt: String,
    pub recent: VecDeque<Event>,
    pub pending_checks: BTreeMap<String, String>,
    // Explicit project obligations are independent of optional model suggestions.
    // Keep them separate so removing a required setting also removes its obligation,
    // without discarding optional checks that the harness already accepted.
    required_checks: BTreeMap<String, String>,
    turn_revision: u64,
    pub obligations: Vec<String>,
    pub checked: BTreeMap<String, u64>,
    pub revision: u64,
    pub continuations: u8,
    pub stopped: bool,
    pub waiting: bool,
    pub active: bool,
    pub last_loop_ms: u64,
    pub last_verification: Option<(u64, String)>,
    pub began_ms: u64,
    seen: VecDeque<String>,
}
impl Tracker {
    /// Returns false for a repeated native notification. IDs are scoped to event kind.
    pub fn event(&mut self, event: Event, metrics: &mut DecisionMetrics, now: u64) -> bool {
        if !event.id.is_empty() {
            let id = format!("{}:{}", event.kind, event.id);
            if self.seen.contains(&id) {
                return false;
            }
            self.seen.push_back(id);
            if self.seen.len() > 2048 {
                self.seen.pop_front();
            }
        }
        match event.kind.as_str() {
            "prompt" => {
                if self.began_ms > 0 {
                    metrics.elapsed_ms += now.saturating_sub(self.began_ms);
                }
                self.generation += 1;
                self.prompt = bounded(&event.text, 8192);
                self.continuations = 0;
                self.stopped = false;
                self.waiting = false;
                self.pending_checks.clear();
                self.required_checks.clear();
                self.turn_revision = self.revision;
                self.obligations.clear();
                self.recent.clear();
                self.active = true;
                self.began_ms = now;
                metrics.turns += 1;
            }
            "stop" => {
                if self.began_ms > 0 {
                    metrics.elapsed_ms += now.saturating_sub(self.began_ms);
                    self.began_ms = 0;
                }
                self.stopped = true;
                self.active = false;
                self.generation += 1;
            }
            "waiting" => {
                if self.began_ms > 0 {
                    metrics.elapsed_ms += now.saturating_sub(self.began_ms);
                    self.began_ms = 0;
                }
                self.waiting = true;
                self.generation += 1;
            }
            "resumed" => {
                if self.waiting && self.active && !self.stopped {
                    self.began_ms = now;
                }
                self.waiting = false;
            }
            "tool" => {
                metrics.tools += 1;
                if event.failed {
                    metrics.failures += 1;
                }
                if event.changed {
                    self.revision += 1;
                    self.generation += 1;
                    self.checked.clear();
                }
                let command = event.command.trim();
                if !command.is_empty() {
                    if event.failed {
                        self.checked.remove(command);
                    } else if command.len() <= 4096 {
                        // Retain successful native commands even before Cide suggests
                        // a check. No shell parsing or inference from output prose.
                        self.checked.insert(command.into(), self.revision);
                        while self.checked.len() > 64 {
                            self.checked.pop_first();
                        }
                    }
                }
                self.pending_checks
                    .retain(|command, _| event.command.trim() != command.trim() || event.failed);
            }
            "complete" => {
                self.active = false;
                if self.began_ms > 0 {
                    metrics.elapsed_ms += now.saturating_sub(self.began_ms);
                    self.began_ms = 0;
                }
                if event.blocked {
                    self.waiting = true;
                }
            }
            "plan" => {
                self.obligations = event
                    .obligations
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .take(24)
                    .map(|s| bounded(&s, 512))
                    .collect();
            }
            "usage" => {
                // Adapters supply native cumulative totals. Repeated usage is never added.
                if let Some(n) = event.input_tokens {
                    metrics.input_tokens = Some(n);
                }
                if let Some(n) = event.output_tokens {
                    metrics.output_tokens = Some(n);
                }
                if let Some(n) = event.cache_tokens {
                    metrics.cache_tokens = Some(n);
                }
                if let Some(n) = event.cost {
                    metrics.cost = Some(n);
                }
            }
            _ => {}
        }
        if matches!(event.kind.as_str(), "tool" | "complete" | "message") {
            let mut event = event;
            event.text = bounded(&event.text, 2048);
            self.recent.push_back(event);
            if self.recent.len() > 12 {
                self.recent.pop_front();
            }
        }
        true
    }
    pub fn state(&self, limit: usize) -> String {
        bounded(&json!({"userRequest":self.prompt,"recent":self.recent,"unverified":self.unfinished_checks(),"requiredChecks":self.required_checks.keys().collect::<Vec<_>>(),"unfinishedPlan":self.obligations}).to_string(), limit)
    }

    /// An explicit required check becomes an obligation after an edit in this user
    /// turn, even when Verification is Off, Observe, below threshold or unavailable.
    /// Refresh from current settings: a removed check must not force a later turn.
    pub fn refresh_required_checks(&mut self, checks: &[DecisionCheck]) {
        self.required_checks = checks
            .iter()
            .take(24)
            .filter(|check| {
                self.revision > self.turn_revision
                    && check.required
                    && !check.command.trim().is_empty()
                    && self.checked.get(check.command.trim()) != Some(&self.revision)
            })
            .map(|check| (check.command.trim().into(), check.description.clone()))
            .collect();
    }

    pub fn unfinished_checks(&self) -> BTreeMap<String, String> {
        let mut checks = self.pending_checks.clone();
        checks.extend(self.required_checks.clone());
        checks
    }
    pub fn loop_due(&self, now: u64, cooldown: u64) -> bool {
        if self.stopped
            || self.waiting
            || !self.active
            || now.saturating_sub(self.last_loop_ms) < cooldown * 1000
        {
            return false;
        }
        let tail: Vec<_> = self
            .recent
            .iter()
            .rev()
            .filter(|e| e.kind == "tool")
            .take(3)
            .collect();
        tail.len() == 3
            && tail.iter().all(|e| {
                e.failed
                    && !e.changed
                    && e.tool == tail[0].tool
                    && failure_signature(e) == failure_signature(tail[0])
            })
    }
    pub fn verification_due(&self, period: &str) -> bool {
        self.active
            && !self.stopped
            && !self.waiting
            && self.revision > 0
            && self
                .last_verification
                .as_ref()
                .is_none_or(|(revision, previous)| *revision != self.revision || previous != period)
    }

    /// Claim once at admission, including Observe, low probability and error outcomes.
    /// A new file revision or settings period permits another attempt.
    pub fn claim_verification(&mut self, period: &str) -> bool {
        if !self.verification_due(period) {
            return false;
        }
        self.last_verification = Some((self.revision, period.into()));
        true
    }
    pub fn completion_due(&self, config: &ProjectDecisionSettings) -> bool {
        !self.active
            && !self.stopped
            && !self.waiting
            && self.continuations < config.max_continuations.min(2)
            && (!self.pending_checks.is_empty()
                || !self.required_checks.is_empty()
                || !self.obligations.is_empty())
    }
}

/// Settings come before discovered checks. Identical commands must never appear
/// as both a required obligation and an optional choice in the same question.
pub fn merge_checks(checks: impl IntoIterator<Item = DecisionCheck>) -> Vec<DecisionCheck> {
    let mut merged: Vec<DecisionCheck> = Vec::new();
    for mut check in checks {
        check.command = check.command.trim().into();
        if check.command.is_empty() {
            continue;
        }
        if let Some(existing) = merged.iter_mut().find(|c| c.command == check.command) {
            existing.required |= check.required;
            if existing.description.is_empty() {
                existing.description = check.description;
            }
        } else if merged.len() < 24 {
            merged.push(check);
        }
    }
    merged
}

fn failure_signature(event: &Event) -> String {
    let command = if event.command.is_empty() {
        event.tool.as_str()
    } else {
        event.command.as_str()
    };
    command
        .split_whitespace()
        .take(4)
        .collect::<Vec<_>>()
        .join(" ")
}

/// A discovery result is a suggestion to the harness, never a shell command run by cide.
pub fn discover_checks(root: &std::path::Path) -> Vec<DecisionCheck> {
    let mut checks = Vec::new();
    let mut add = |command: &str, description: &str| {
        checks.push(DecisionCheck {
            command: command.into(),
            description: description.into(),
            required: false,
        })
    };
    if root.join("Cargo.toml").is_file() {
        add("cargo test --locked", "Rust tests");
        add("cargo check --locked", "Rust type check");
    }
    if root.join("go.mod").is_file() {
        add("go test ./...", "Go tests");
    }
    if root.join("pytest.ini").is_file() || root.join("pyproject.toml").is_file() {
        add("python -m pytest", "Python tests");
    }
    if std::fs::metadata(root.join("package.json"))
        .is_ok_and(|m| m.is_file() && m.len() <= 256 * 1024)
        && let Ok(bytes) = std::fs::read(root.join("package.json"))
        && bytes.len() <= 256 * 1024
        && let Ok(package) = serde_json::from_slice::<Value>(&bytes)
        && let Some(scripts) = package["scripts"].as_object()
    {
        let manager = if root.join("pnpm-lock.yaml").exists() {
            "pnpm"
        } else if root.join("yarn.lock").exists() {
            "yarn"
        } else {
            "npm run"
        };
        for (name, _) in scripts {
            if checks.len() >= 24 {
                break;
            }
            if (name.starts_with("test")
                || name.starts_with("check")
                || name.starts_with("lint")
                || name == "typecheck")
                && name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b":_-".contains(&c))
            {
                checks.push(DecisionCheck {
                    command: format!("{manager} {name}"),
                    description: format!("package.json script {name}"),
                    required: false,
                });
            }
        }
    }
    checks
}

#[derive(Default)]
struct Aggregate {
    decisions: u64,
    applied: u64,
    observed: u64,
    errors: u64,
    latencies: VecDeque<u64>,
}
#[derive(Default)]
pub struct Journal {
    pub revision: u64,
    pub dropped: u64,
    bytes: usize,
    history: VecDeque<DecisionRecord>,
    aggregates: BTreeMap<DecisionScenario, Aggregate>,
    pub sessions: BTreeMap<String, DecisionMetrics>,
}
impl Journal {
    pub fn permits(&self, id: u64, session: &str, period: &str) -> bool {
        self.history.iter().rev().any(|r| {
            r.id == id
                && r.session == session
                && r.period == period
                && r.mode == DecisionMode::Automatic
                && matches!(r.status.as_str(), "proposed" | "delivered")
        })
    }
    pub fn push(&mut self, record: DecisionRecord) {
        let a = self.aggregates.entry(record.scenario).or_default();
        a.decisions += 1;
        if record.status == "applied" {
            a.applied += 1;
        }
        if record.status == "observed" {
            a.observed += 1;
        }
        if record.status == "error" {
            a.errors += 1;
        }
        a.latencies.push_back(record.latency_ms);
        if a.latencies.len() > 2048 {
            a.latencies.pop_front();
        }
        self.bytes += serde_json::to_vec(&record).map_or(0, |v| v.len());
        self.history.push_back(record);
        self.trim();
        self.revision += 1;
    }
    fn trim(&mut self) {
        while self.history.len() > HISTORY_LIMIT || self.bytes > HISTORY_BYTES {
            if let Some(old) = self.history.pop_front() {
                self.bytes = self
                    .bytes
                    .saturating_sub(serde_json::to_vec(&old).map_or(0, |v| v.len()));
                self.dropped += 1;
            }
        }
    }
    fn resized(&mut self, before: usize, after: usize) {
        self.bytes = self.bytes.saturating_sub(before) + after;
        self.trim();
        self.revision += 1;
    }
    pub fn observe(&mut self, session: &str, event: &Event) {
        let mut added = 0;
        for r in self
            .history
            .iter_mut()
            .rev()
            .take(128)
            .filter(|r| r.session == session && r.status == "applied" && r.outcome.is_empty())
        {
            let outcome = match (r.scenario, event.kind.as_str()) {
                (DecisionScenario::Loop, "tool") => Some(if event.failed {
                    "The next tool action failed"
                } else {
                    "The next tool action succeeded"
                }),
                (DecisionScenario::Verification, "tool")
                    if !event.failed
                        && r.action
                            .lines()
                            .filter_map(|l| l.strip_prefix("- "))
                            .any(|c| {
                                event.command.trim() == c.trim_end_matches(" (required)").trim()
                            }) =>
                {
                    Some("A selected check completed successfully")
                }
                (DecisionScenario::Completion, "complete") => Some(if event.blocked {
                    "The follow-up stopped or encountered a blocker"
                } else {
                    "The follow-up turn finished"
                }),
                (DecisionScenario::Model, "usage") => {
                    Some("Usage was reported after the profile selection")
                }
                _ => None,
            };
            if let Some(outcome) = outcome {
                r.outcome = outcome.into();
                added += outcome.len();
            }
        }
        if added > 0 {
            self.bytes += added;
            self.trim();
            self.revision += 1;
        }
    }
    pub fn action(&mut self, id: u64, text: &str) {
        if let Some(r) = self.history.iter_mut().find(|r| r.id == id) {
            let before = serde_json::to_vec(r).map_or(0, |v| v.len());
            r.action = bounded(text, 4096);
            let after = serde_json::to_vec(r).map_or(0, |v| v.len());
            self.resized(before, after);
        }
    }
    pub fn feedback(&mut self, id: u64, helpful: bool) {
        if let Some(r) = self.history.iter_mut().find(|r| r.id == id) {
            let before = serde_json::to_vec(r).map_or(0, |v| v.len());
            r.feedback = Some(helpful);
            let after = serde_json::to_vec(r).map_or(0, |v| v.len());
            self.resized(before, after);
        }
    }
    pub fn status(&mut self, id: u64, status: &str, reason: &str) {
        if let Some(r) = self.history.iter_mut().find(|r| r.id == id) {
            if r.status == "applied" {
                return;
            }
            let before = serde_json::to_vec(r).map_or(0, |v| v.len());
            if status == "applied" {
                self.aggregates.entry(r.scenario).or_default().applied += 1;
            }
            r.status = status.into();
            r.reason = bounded(reason, 1024);
            let after = serde_json::to_vec(r).map_or(0, |v| v.len());
            self.resized(before, after);
        }
    }
    pub fn snapshot(
        &self,
        offset: usize,
        limit: usize,
        scenario: Option<DecisionScenario>,
        project: Option<ProjectId>,
    ) -> DecisionSnapshot {
        let filtered: Vec<_> = self
            .history
            .iter()
            .rev()
            .filter(|r| {
                scenario.is_none_or(|s| s == r.scenario) && project.is_none_or(|p| p == r.project)
            })
            .collect();
        let scenarios = DecisionScenario::ALL
            .into_iter()
            .map(|s| {
                let mut out = DecisionSummary {
                    scenario: Some(s),
                    ..Default::default()
                };
                if let Some(a) = self.aggregates.get(&s) {
                    out.decisions = a.decisions;
                    out.applied = a.applied;
                    out.observed = a.observed;
                    out.errors = a.errors;
                    let mut latencies: Vec<_> = a.latencies.iter().copied().collect();
                    latencies.sort_unstable();
                    if !latencies.is_empty() {
                        out.latency_p50_ms = latencies[(latencies.len() - 1) * 50 / 100];
                        out.latency_p95_ms = latencies[(latencies.len() - 1) * 95 / 100];
                    }
                }
                out
            })
            .collect();
        DecisionSnapshot {
            revision: self.revision,
            dropped: self.dropped,
            sessions: self.sessions.values().cloned().collect(),
            scenarios,
            total: filtered.len(),
            history: filtered
                .into_iter()
                .skip(offset)
                .take(limit.min(500))
                .cloned()
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loop_answer(inspect: f64, change: f64, keep: f64) -> Answer {
        let probabilities: BTreeMap<String, f64> = BTreeMap::from([
            ("inspect".into(), inspect),
            ("change".into(), change),
            ("keep".into(), keep),
        ]);
        let choice = probabilities
            .iter()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0
            .clone();
        let response = json!({"answers":{"loop":{"type":"choice","choice":choice,"probabilities":probabilities}}});
        answers(
            &response,
            &BTreeMap::from([("loop".into(), loop_question())]),
        )
        .unwrap()
        .remove("loop")
        .unwrap()
    }

    #[test]
    fn split_intervention_support_is_gated_once_then_selects_advice() {
        let answer = loop_answer(0.38, 0.32, 0.30);
        let selection = select(DecisionScenario::Loop, &answer, 50);
        assert_eq!(selection.status, "proposed");
        assert_eq!(selection.choice, "inspect");
        let gate = selection.intervention.unwrap();
        assert!(gate.passed);
        assert!((gate.probability - 0.70).abs() < 1e-12);
        assert!((gate.choices["Inspect diagnostics and source"] - 38. / 70.).abs() < 1e-12);
        assert!((gate.choices.values().sum::<f64>() - 1.).abs() < 1e-12);
        assert_eq!(
            select(DecisionScenario::Loop, &answer, 70).status,
            "proposed"
        );
        let blocked = select(DecisionScenario::Loop, &answer, 80);
        assert_eq!(blocked.status, "belowThreshold");
        assert!(!blocked.intervention.unwrap().passed);
        // The aggregate can win even when keep is the single most likely choice.
        let answer = loop_answer(0.27, 0.33, 0.40);
        assert_eq!(answer.choice, "keep");
        let selection = select(DecisionScenario::Loop, &answer, 50);
        assert_eq!(selection.status, "proposed");
        assert_eq!(selection.choice, "change");
    }

    #[test]
    fn intervention_ties_noop_and_rounding_are_explicit() {
        for (inspect, change, keep) in [(0., 0., 1.), (0.25, 0.25, 0.5), (0.2, 0.3, 0.5)] {
            let selection = select(
                DecisionScenario::Loop,
                &loop_answer(inspect, change, keep),
                50,
            );
            assert_eq!(selection.choice, "keep");
            assert_eq!(selection.status, "unchanged");
            assert!(!selection.intervention.unwrap().passed);
        }
        let selection = select(DecisionScenario::Loop, &loop_answer(0.35, 0.35, 0.30), 50);
        assert_eq!(selection.choice, "inspect");
        let rounded = select(
            DecisionScenario::Loop,
            &loop_answer(0.333, 0.333, 0.333),
            50,
        );
        assert!((rounded.intervention.unwrap().probability - 2. / 3.).abs() < 1e-12);
        let ordinary = select(
            DecisionScenario::Research,
            &loop_answer(0.38, 0.32, 0.30),
            50,
        );
        assert_eq!(ordinary.status, "belowThreshold");
        assert!(ordinary.intervention.is_none());
    }

    #[test]
    fn verification_is_claimed_once_until_revision_or_settings_change() {
        let mut tracker = Tracker::default();
        let mut metrics = DecisionMetrics::default();
        tracker.event(
            Event {
                kind: "prompt".into(),
                ..Default::default()
            },
            &mut metrics,
            1,
        );
        assert!(!tracker.claim_verification("p"));
        tracker.event(
            Event {
                kind: "tool".into(),
                changed: true,
                ..Default::default()
            },
            &mut metrics,
            2,
        );
        assert!(tracker.claim_verification("p"));
        // Low probability, observation and errors all leave this same admission
        // marker intact; unrelated commands and a new prompt must not retry it.
        tracker.event(
            Event {
                kind: "tool".into(),
                command: "sleep 5".into(),
                ..Default::default()
            },
            &mut metrics,
            3,
        );
        tracker.event(
            Event {
                kind: "prompt".into(),
                ..Default::default()
            },
            &mut metrics,
            4,
        );
        assert!(!tracker.claim_verification("p"));
        assert!(tracker.claim_verification("new-settings"));
        tracker.event(
            Event {
                kind: "tool".into(),
                changed: true,
                ..Default::default()
            },
            &mut metrics,
            5,
        );
        tracker.event(
            Event {
                kind: "waiting".into(),
                ..Default::default()
            },
            &mut metrics,
            6,
        );
        assert!(!tracker.claim_verification("new-settings"));
        tracker.event(
            Event {
                kind: "resumed".into(),
                ..Default::default()
            },
            &mut metrics,
            7,
        );
        assert!(tracker.claim_verification("new-settings"));
        tracker.event(
            Event {
                kind: "stop".into(),
                ..Default::default()
            },
            &mut metrics,
            8,
        );
        assert!(!tracker.claim_verification("another-settings"));
    }

    #[test]
    fn independent_checks_are_bounded_invalidated_on_failure_and_cleared_on_edit() {
        let mut tracker = Tracker::default();
        let mut metrics = DecisionMetrics::default();
        let mut event = Event {
            kind: "tool".into(),
            command: " npm run check ".into(),
            ..Default::default()
        };
        tracker.event(event.clone(), &mut metrics, 1);
        assert_eq!(tracker.checked.get("npm run check"), Some(&0));
        event.failed = true;
        tracker.event(event.clone(), &mut metrics, 2);
        assert!(!tracker.checked.contains_key("npm run check"));
        event.failed = false;
        tracker.event(event, &mut metrics, 3);
        for n in 0..80 {
            tracker.event(
                Event {
                    kind: "tool".into(),
                    command: format!("check{n}"),
                    ..Default::default()
                },
                &mut metrics,
                4,
            );
        }
        assert_eq!(tracker.checked.len(), 64);
        tracker.event(
            Event {
                kind: "tool".into(),
                changed: true,
                ..Default::default()
            },
            &mut metrics,
            5,
        );
        assert!(tracker.checked.is_empty());
    }

    #[test]
    fn required_check_reaches_completion_without_a_verification_suggestion() {
        let check = DecisionCheck {
            command: "npm run check".into(),
            description: "Validate the app".into(),
            required: true,
        };
        let mut config = ProjectDecisionSettings {
            enabled: true,
            checks: vec![check],
            ..Default::default()
        };
        for verification_mode in [
            DecisionMode::Off,
            DecisionMode::Observe,
            DecisionMode::Automatic,
        ] {
            config
                .scenarios
                .get_mut(&DecisionScenario::Verification)
                .unwrap()
                .mode = verification_mode;
            let mut tracker = Tracker::default();
            let mut metrics = DecisionMetrics::default();
            for kind in ["prompt", "tool", "complete"] {
                tracker.event(
                    Event {
                        kind: kind.into(),
                        changed: kind == "tool",
                        ..Default::default()
                    },
                    &mut metrics,
                    1,
                );
                tracker.refresh_required_checks(&config.checks);
            }
            // No optional verification was accepted and no plan was created.
            assert!(tracker.pending_checks.is_empty());
            assert!(tracker.obligations.is_empty());
            assert!(tracker.completion_due(&config));
            let state: Value = serde_json::from_str(&tracker.state(24576)).unwrap();
            assert_eq!(state["requiredChecks"], json!(["npm run check"]));
            assert_eq!(state["unverified"]["npm run check"], "Validate the app");
            // The completion decision still has its own mode and threshold.
            assert_eq!(
                mode(&config, "treatment", DecisionScenario::Completion),
                DecisionMode::Automatic
            );
            assert_eq!(config.rule(DecisionScenario::Completion).threshold, 90);
            tracker.continuations = 2;
            assert!(!tracker.completion_due(&config));
            tracker.continuations = 0;
            tracker.waiting = true;
            assert!(!tracker.completion_due(&config));
            tracker.waiting = false;
            tracker.stopped = true;
            assert!(!tracker.completion_due(&config));
        }
    }

    #[test]
    fn required_checks_follow_edits_success_settings_and_user_turns() {
        let config = ProjectDecisionSettings {
            checks: vec![DecisionCheck {
                command: " npm run check ".into(),
                description: String::new(),
                required: true,
            }],
            ..Default::default()
        };
        let mut tracker = Tracker::default();
        let mut metrics = DecisionMetrics::default();
        for (event, expected) in [
            (
                Event {
                    kind: "prompt".into(),
                    ..Default::default()
                },
                false,
            ),
            (
                Event {
                    kind: "tool".into(),
                    changed: true,
                    ..Default::default()
                },
                true,
            ),
            (
                Event {
                    kind: "tool".into(),
                    command: "npm run check".into(),
                    failed: true,
                    ..Default::default()
                },
                true,
            ),
            (
                Event {
                    kind: "tool".into(),
                    command: "npm run check".into(),
                    ..Default::default()
                },
                false,
            ),
            (
                Event {
                    kind: "tool".into(),
                    changed: true,
                    ..Default::default()
                },
                true,
            ),
            (
                Event {
                    kind: "prompt".into(),
                    ..Default::default()
                },
                false,
            ),
            (
                Event {
                    kind: "tool".into(),
                    command: "cat index.html".into(),
                    ..Default::default()
                },
                false,
            ),
            (
                Event {
                    kind: "complete".into(),
                    ..Default::default()
                },
                false,
            ),
        ] {
            tracker.event(event, &mut metrics, 1);
            tracker.refresh_required_checks(&config.checks);
            assert_eq!(
                tracker.unfinished_checks().contains_key("npm run check"),
                expected
            );
        }
        assert!(!tracker.completion_due(&config));
        tracker.event(
            Event {
                kind: "tool".into(),
                changed: true,
                ..Default::default()
            },
            &mut metrics,
            2,
        );
        tracker.refresh_required_checks(&config.checks);
        assert!(tracker.completion_due(&config));
        tracker
            .pending_checks
            .insert("npm test".into(), "Accepted optional check".into());
        tracker.refresh_required_checks(&[]);
        assert!(!tracker.unfinished_checks().contains_key("npm run check"));
        assert!(tracker.unfinished_checks().contains_key("npm test"));
    }

    #[test]
    fn required_and_discovered_check_merge_into_one_required_command() {
        let merged = merge_checks([
            DecisionCheck {
                command: " npm run check ".into(),
                description: String::new(),
                required: true,
            },
            DecisionCheck {
                command: "npm run check".into(),
                description: "package.json script check".into(),
                required: false,
            },
            DecisionCheck {
                command: "npm test".into(),
                description: "tests".into(),
                required: false,
            },
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].command, "npm run check");
        assert!(merged[0].required);
        assert_eq!(merged[0].description, "package.json script check");
        assert!(!merged[1].required);
    }

    #[test]
    fn probability_not_confidence_and_unknown_choices_fail() {
        let q = BTreeMap::from([(
            "route".into(),
            Question::choice(
                "route",
                [("a".into(), "A".into()), ("b".into(), "B".into())],
            ),
        )]);
        let good = json!({"answers":{"route":{"type":"choice","choice":"a","probabilities":{"a":0.6,"b":0.4},"confidence":0.99}}});
        assert_eq!(answers(&good, &q).unwrap()["route"].probability(), 0.6);
        let mut bad = good.clone();
        bad["answers"]["route"]["choice"] = json!("invented");
        assert!(answers(&bad, &q).is_err());
        let mut bad = good;
        bad["answers"]["route"]["probabilities"]["b"] = json!(1.1);
        assert!(answers(&bad, &q).is_err());
    }
    #[test]
    fn stopped_and_waiting_never_continue() {
        let mut t = Tracker::default();
        let mut m = DecisionMetrics::default();
        t.pending_checks
            .insert("test".into(), "missing test".into());
        assert!(t.completion_due(&ProjectDecisionSettings::default()));
        t.event(
            Event {
                kind: "stop".into(),
                ..Default::default()
            },
            &mut m,
            10,
        );
        assert!(!t.completion_due(&ProjectDecisionSettings::default()));
        t.stopped = false;
        t.waiting = true;
        assert!(!t.completion_due(&ProjectDecisionSettings::default()));
    }
    #[test]
    fn baseline_and_control_never_call_selected_scenarios() {
        let mut p = ProjectDecisionSettings::default();
        assert_eq!(
            mode(&p, "treatment", DecisionScenario::Loop),
            DecisionMode::Off
        );
        p.enabled = true;
        p.ab_enabled = true;
        p.ab_scenario = Some(DecisionScenario::Loop);
        assert_eq!(
            mode(&p, "control", DecisionScenario::Loop),
            DecisionMode::Off
        );
        assert_eq!(
            mode(&p, "control", DecisionScenario::Context),
            DecisionMode::Automatic
        );
        assert_eq!(group(&p, "parent"), group(&p, "parent"));
    }
    #[test]
    fn dedup_usage_and_three_failures() {
        let mut t = Tracker::default();
        let mut m = DecisionMetrics::default();
        t.event(
            Event {
                kind: "prompt".into(),
                ..Default::default()
            },
            &mut m,
            1,
        );
        for i in 0..3 {
            assert!(t.event(
                Event {
                    kind: "tool".into(),
                    id: i.to_string(),
                    tool: "shell".into(),
                    failed: true,
                    ..Default::default()
                },
                &mut m,
                100
            ));
        }
        assert!(t.loop_due(31_000, 30));
        assert!(!t.event(
            Event {
                kind: "tool".into(),
                id: "2".into(),
                ..Default::default()
            },
            &mut m,
            100
        ));
        for _ in 0..2 {
            t.event(
                Event {
                    kind: "usage".into(),
                    input_tokens: Some(100),
                    ..Default::default()
                },
                &mut m,
                100,
            );
        }
        assert_eq!(m.input_tokens, Some(100));
        assert_eq!(m.tools, 3);
        t.event(
            Event {
                kind: "tool".into(),
                changed: true,
                ..Default::default()
            },
            &mut m,
            100,
        );
        assert!(!t.loop_due(31_000, 30));
    }
    #[test]
    fn utf8_bound_and_limits() {
        assert_eq!(bounded("абв", 3), "а");
        assert!(validate_project(&ProjectDecisionSettings::default()).is_ok());
        assert!(validate_connection(&DecisionSettings::default()).is_ok());
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;
    fn record(id: u64) -> DecisionRecord {
        DecisionRecord {
            id,
            at_ms: id,
            project: ProjectId::new(),
            session: "s".into(),
            harness: "codex".into(),
            scenario: DecisionScenario::Loop,
            period: "p".into(),
            group: "treatment".into(),
            mode: DecisionMode::Automatic,
            question: "retry?".into(),
            state: "three failures".into(),
            probabilities: BTreeMap::new(),
            intervention: None,
            choice: "inspect".into(),
            status: "proposed".into(),
            reason: String::new(),
            action: String::new(),
            outcome: String::new(),
            latency_ms: 20,
            input_tokens: Some(12),
            feedback: None,
        }
    }
    #[test]
    fn journal_retains_aggregates_after_evicting_detail_and_ack_is_idempotent() {
        let mut j = Journal::default();
        for i in 0..=HISTORY_LIMIT as u64 {
            j.push(record(i));
        }
        let s = j.snapshot(0, 500, None, None);
        assert_eq!(s.dropped, 1);
        assert_eq!(s.total, HISTORY_LIMIT);
        assert_eq!(s.history.len(), 500);
        assert_eq!(s.scenarios[0].decisions, HISTORY_LIMIT as u64 + 1);
        j.status(HISTORY_LIMIT as u64, "applied", "accepted");
        j.status(HISTORY_LIMIT as u64, "applied", "accepted twice");
        j.feedback(HISTORY_LIMIT as u64, true);
        let s = j.snapshot(0, 1, None, None);
        assert_eq!(s.scenarios[0].applied, 1);
        assert_eq!(s.history[0].feedback, Some(true));
        assert_eq!(Journal::default().snapshot(0, 500, None, None).total, 0);
    }
    #[test]
    fn queued_delivery_requires_its_original_session_and_settings_period() {
        let mut j = Journal::default();
        j.push(record(7));
        assert!(j.permits(7, "s", "p"));
        assert!(!j.permits(7, "other", "p"));
        assert!(!j.permits(7, "s", "new-settings"));
        j.status(7, "applied", "native acknowledgement");
        assert!(!j.permits(7, "s", "p"));
        let other_window = Journal::default();
        assert!(!other_window.permits(7, "s", "p"));
    }
    #[test]
    fn two_policy_steps_remain_one_record_and_observe_never_permits_delivery() {
        let mut journal = Journal::default();
        let mut r = record(7);
        r.intervention = Some(DecisionIntervention {
            probability: 0.7,
            threshold: 50,
            passed: true,
            choices: BTreeMap::from([("inspect".into(), 38. / 70.), ("change".into(), 32. / 70.)]),
        });
        r.mode = DecisionMode::Observe;
        r.status = "observed".into();
        journal.push(r);
        assert!(!journal.permits(7, "s", "p"));
        let snapshot = journal.snapshot(0, 10, None, None);
        assert_eq!(snapshot.history.len(), 1);
        assert_eq!(snapshot.scenarios[0].decisions, 1);
        assert_eq!(snapshot.scenarios[0].observed, 1);
        assert_eq!(snapshot.scenarios[0].applied, 0);
        assert!(snapshot.history[0].intervention.as_ref().unwrap().passed);
    }
    #[test]
    fn settings_are_local_and_configuration_changes_separate_periods() {
        let global = Settings::default();
        let p = ProjectHarnessSettings {
            decision: Some(ProjectDecisionSettings {
                enabled: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(p.resolve(&global).decision, global.decision);
        let defaults = ProjectDecisionSettings::default();
        let enabled = p.decision.unwrap();
        assert_ne!(
            period(&global.decision, &defaults),
            period(&global.decision, &enabled)
        );
        assert!(
            !ProjectHarnessSettings::default()
                .decision
                .unwrap_or_default()
                .enabled
        );
    }
    #[test]
    fn successful_checks_only_cover_the_same_revision() {
        let mut t = Tracker::default();
        let mut m = DecisionMetrics::default();
        t.event(
            Event {
                kind: "prompt".into(),
                ..Default::default()
            },
            &mut m,
            1,
        );
        t.checked.insert("cargo test".into(), u64::MAX);
        t.pending_checks.insert("cargo test".into(), "tests".into());
        t.event(
            Event {
                kind: "tool".into(),
                text: "cargo test".into(),
                command: "cargo test".into(),
                failed: true,
                ..Default::default()
            },
            &mut m,
            2,
        );
        assert!(!t.pending_checks.is_empty());
        t.event(
            Event {
                kind: "tool".into(),
                text: "cargo test".into(),
                command: "cargo test".into(),
                ..Default::default()
            },
            &mut m,
            3,
        );
        assert!(t.pending_checks.is_empty());
        assert_eq!(t.checked["cargo test"], t.revision);
        t.event(
            Event {
                kind: "tool".into(),
                changed: true,
                ..Default::default()
            },
            &mut m,
            4,
        );
        assert_ne!(t.checked.get("cargo test"), Some(&t.revision));
    }
    #[test]
    fn complete_plan_and_continuation_budget_gate_completion() {
        let mut t = Tracker::default();
        let mut m = DecisionMetrics::default();
        t.event(
            Event {
                kind: "plan".into(),
                obligations: Some(vec!["Fix failing test".into()]),
                ..Default::default()
            },
            &mut m,
            1,
        );
        assert!(t.completion_due(&ProjectDecisionSettings::default()));
        t.continuations = 2;
        assert!(!t.completion_due(&ProjectDecisionSettings::default()));
        t.continuations = 0;
        t.event(
            Event {
                kind: "plan".into(),
                obligations: Some(vec![]),
                ..Default::default()
            },
            &mut m,
            2,
        );
        assert!(!t.completion_due(&ProjectDecisionSettings::default()));
    }

    #[test]
    fn observed_active_time_excludes_permission_waits_and_includes_interrupted_work() {
        let mut t = Tracker::default();
        let mut m = DecisionMetrics::default();
        for (kind, at) in [
            ("prompt", 100),
            ("waiting", 200),
            ("waiting", 300),
            ("resumed", 900),
            ("complete", 950),
            ("complete", 960),
        ] {
            t.event(
                Event {
                    kind: kind.into(),
                    ..Default::default()
                },
                &mut m,
                at,
            );
        }
        assert_eq!(m.elapsed_ms, 150);
        assert_eq!(m.turns, 1);
        t.event(
            Event {
                kind: "prompt".into(),
                ..Default::default()
            },
            &mut m,
            1000,
        );
        t.event(
            Event {
                kind: "stop".into(),
                ..Default::default()
            },
            &mut m,
            1200,
        );
        assert_eq!(m.elapsed_ms, 350);
        assert!(!t.completion_due(&ProjectDecisionSettings::default()));
    }
}
