//! Local recovery history, independent of Git and transient project identifiers.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const HISTORY_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(transparent)]
#[ts(export)]
pub struct HistoryRootId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(transparent)]
#[ts(export)]
pub struct LocalRevisionId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum HistorySource {
    Baseline,
    Reopen,
    CideSave,
    External,
    Restore,
    FileOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", tag = "kind")]
#[ts(export)]
pub enum HistoryContent {
    Absent,
    Content { digest: String, bytes: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LocalRevision {
    pub id: LocalRevisionId,
    pub sequence: u32,
    #[ts(type = "number")]
    pub observed_ms: u64,
    pub path: String,
    pub lifecycle: String,
    pub source: HistorySource,
    pub prior_path: Option<String>,
    pub content: HistoryContent,
    pub unavailable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields, default)]
#[ts(export)]
pub struct HistoryPolicy {
    pub enabled: bool,
    pub retention_days: u32,
    pub budget_bytes: u32,
    pub exclusions: Vec<String>,
}
impl Default for HistoryPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            retention_days: 7,
            budget_bytes: 256 * 1024 * 1024,
            exclusions: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryCoverage {
    pub baseline_running: bool,
    pub scanned_files: u32,
    pub captured_files: u32,
    pub notices: Vec<String>,
    pub degraded: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryStatus {
    pub root: HistoryRootId,
    pub canonical_root: String,
    pub generation: u32,
    pub revision: u32,
    pub used_bytes: u32,
    pub policy: HistoryPolicy,
    pub coverage: HistoryCoverage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryPath {
    pub path: String,
    pub deleted: bool,
    pub latest: LocalRevisionId,
    pub excluded: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryPathPage {
    pub paths: Vec<HistoryPath>,
    pub next: Option<String>,
    pub generation: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryRevisionPage {
    pub revisions: Vec<LocalRevision>,
    pub next: Option<u32>,
    pub generation: u32,
    pub excluded: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistorySide {
    pub revision: Option<LocalRevisionId>,
    #[ts(type = "number | null")]
    pub observed_ms: Option<u64>,
    pub content: HistoryContent,
    pub text: Option<String>,
    pub unavailable: Option<String>,
}

/// Digest and canonical destination bind a confirmation to the disk state that was previewed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct HistoryCompareToken {
    pub generation: u32,
    pub path: String,
    pub canonical_target: String,
    pub current: HistoryContent,
    pub target: LocalRevisionId,
    pub lifecycle: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryComparison {
    pub left: HistorySide,
    pub right: HistorySide,
    pub token: HistoryCompareToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryRestoreResult {
    pub restored: bool,
    pub safety_revision: LocalRevisionId,
    pub degraded: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryInvalidation {
    pub root: HistoryRootId,
    pub paths: Vec<String>,
    pub generation: u32,
    pub revision: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryEditorRegistration {
    pub window: String,
    pub editor: String,
    pub mount: String,
    pub canonical_path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum HistoryGatePhase {
    Prepare,
    Release,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryEditorGate {
    pub request: String,
    pub canonical_path: String,
    pub editors: Vec<HistoryEditorRegistration>,
    pub phase: HistoryGatePhase,
    pub restored: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LocalHistoryView {
    pub root: HistoryRootId,
    /// None opens the project browser, including deleted paths.
    pub path: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_policy_uses_defaults() {
        assert_eq!(
            serde_json::from_str::<HistoryPolicy>("{}").unwrap(),
            HistoryPolicy::default()
        );
    }

    #[test]
    fn old_tool_window_has_no_local_history_views() {
        let mut old = serde_json::to_value(crate::ToolWindowState::default()).unwrap();
        old.as_object_mut().unwrap().remove("localHistory");
        old.as_object_mut().unwrap().remove("activeLocalHistory");
        let restored: crate::ToolWindowState = serde_json::from_value(old).unwrap();
        assert!(restored.local_history.is_empty());
        assert!(restored.active_local_history.is_none());
    }
}
