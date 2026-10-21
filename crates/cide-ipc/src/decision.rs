//! Experimental decision assistance. Only configuration is durable; records belong to a window.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct DecisionSettings {
    pub url: String,
    pub model: String,
    #[ts(type = "number")]
    pub timeout_ms: u64,
    pub profiles: Vec<DecisionProfile>,
}
impl Default for DecisionSettings {
    fn default() -> Self {
        Self {
            url: "http://127.0.0.1:11434".into(),
            model: "d1".into(),
            timeout_ms: 2000,
            profiles: vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DecisionProfile {
    pub id: String,
    pub harness: crate::ConsoleHarness,
    pub model: String,
    pub effort: Option<String>,
    pub description: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DecisionScenario {
    Loop,
    Context,
    Research,
    Verification,
    Completion,
    Model,
}
impl DecisionScenario {
    pub const ALL: [Self; 6] = [
        Self::Loop,
        Self::Context,
        Self::Research,
        Self::Verification,
        Self::Completion,
        Self::Model,
    ];
    pub fn key(self) -> &'static str {
        match self {
            Self::Loop => "loop",
            Self::Context => "context",
            Self::Research => "research",
            Self::Verification => "verification",
            Self::Completion => "completion",
            Self::Model => "model",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum DecisionMode {
    Off,
    Observe,
    #[default]
    Automatic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct DecisionRule {
    pub mode: DecisionMode,
    /// Percent probability, not the API's confidence field.
    pub threshold: u8,
}
impl Default for DecisionRule {
    fn default() -> Self {
        Self {
            mode: DecisionMode::Automatic,
            threshold: 80,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ProjectDecisionSettings {
    pub enabled: bool,
    pub scenarios: std::collections::BTreeMap<DecisionScenario, DecisionRule>,
    pub profiles: Vec<String>,
    pub checks: Vec<DecisionCheck>,
    pub discover_checks: bool,
    pub ab_enabled: bool,
    pub ab_scenario: Option<DecisionScenario>,
    pub max_continuations: u8,
    #[ts(type = "number")]
    pub cooldown_secs: u64,
    pub max_input_bytes: usize,
    pub context_limit: usize,
}
impl Default for ProjectDecisionSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            scenarios: DecisionScenario::ALL
                .into_iter()
                .map(|s| {
                    (
                        s,
                        DecisionRule {
                            threshold: if s == DecisionScenario::Completion {
                                90
                            } else {
                                80
                            },
                            ..Default::default()
                        },
                    )
                })
                .collect(),
            profiles: vec![],
            checks: vec![],
            discover_checks: true,
            ab_enabled: false,
            ab_scenario: None,
            max_continuations: 2,
            cooldown_secs: 30,
            max_input_bytes: 24 * 1024,
            context_limit: 8,
        }
    }
}
impl ProjectDecisionSettings {
    pub fn rule(&self, scenario: DecisionScenario) -> DecisionRule {
        self.scenarios
            .get(&scenario)
            .cloned()
            .unwrap_or_else(|| DecisionRule {
                threshold: if scenario == DecisionScenario::Completion {
                    90
                } else {
                    80
                },
                ..Default::default()
            })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DecisionCheck {
    pub command: String,
    pub description: String,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DecisionRecord {
    #[ts(type = "number")]
    pub id: u64,
    #[ts(type = "number")]
    pub at_ms: u64,
    pub project: crate::ProjectId,
    pub session: String,
    pub harness: String,
    pub scenario: DecisionScenario,
    pub period: String,
    pub group: String,
    pub mode: DecisionMode,
    pub question: String,
    pub state: String,
    pub probabilities: std::collections::BTreeMap<String, f64>,
    #[serde(default)]
    pub intervention: Option<DecisionIntervention>,
    pub choice: String,
    pub status: String,
    pub reason: String,
    pub action: String,
    pub outcome: String,
    #[ts(type = "number")]
    pub latency_ms: u64,
    #[ts(type = "number | null")]
    pub input_tokens: Option<u64>,
    pub feedback: Option<bool>,
}

/// Derived from one mutually exclusive distribution; choices are conditional on intervention.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DecisionIntervention {
    pub probability: f64,
    pub threshold: u8,
    pub passed: bool,
    pub choices: std::collections::BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DecisionMetrics {
    pub session: String,
    pub project: Option<crate::ProjectId>,
    pub harness: String,
    pub group: String,
    pub period: String,
    pub enabled: bool,
    pub pinned: bool,
    pub available: bool,
    pub note: String,
    #[ts(type = "number")]
    pub turns: u64,
    #[ts(type = "number")]
    pub tools: u64,
    #[ts(type = "number")]
    pub failures: u64,
    #[ts(type = "number")]
    pub continuations: u64,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    #[ts(type = "number | null")]
    pub input_tokens: Option<u64>,
    #[ts(type = "number | null")]
    pub output_tokens: Option<u64>,
    #[ts(type = "number | null")]
    pub cache_tokens: Option<u64>,
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DecisionSummary {
    pub scenario: Option<DecisionScenario>,
    #[ts(type = "number")]
    pub decisions: u64,
    #[ts(type = "number")]
    pub applied: u64,
    #[ts(type = "number")]
    pub observed: u64,
    #[ts(type = "number")]
    pub errors: u64,
    #[ts(type = "number")]
    pub latency_p50_ms: u64,
    #[ts(type = "number")]
    pub latency_p95_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DecisionSnapshot {
    #[ts(type = "number")]
    pub revision: u64,
    #[ts(type = "number")]
    pub dropped: u64,
    pub sessions: Vec<DecisionMetrics>,
    pub scenarios: Vec<DecisionSummary>,
    pub history: Vec<DecisionRecord>,
    pub total: usize,
}
