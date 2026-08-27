//! The shapes the `openspec` CLI emits, and the translation into cide's own. (M28)
//!
//! # Why these are here and not in `cide-ipc`
//!
//! They are **npm's contract, not cide's**. `cide_ipc::spec` is what the panels are built
//! against and what `ts-rs` exports; these are what version 1.8.0 of `@fission-ai/openspec`
//! happens to print. Keeping them apart is what lets an upstream field rename land in this one
//! file — with a test naming the version that changed — instead of rippling through the wire, the
//! generated TypeScript and three React components.
//!
//! Everything here is `Deserialize`-only and **tolerant**: every field this build does not need
//! is simply absent from the struct, and everything optional carries `#[serde(default)]`. The
//! posture is deliberate and is the opposite of `cide-ipc`'s `deny_unknown_fields`. A document
//! from a newer CLI with three fields cide has never heard of must still produce a board; a
//! *missing* field that cide does need is the case that refuses, and it refuses in
//! [`crate::cli::parse`] with the command named.

use std::path::PathBuf;

use serde::Deserialize;

/// `{"path": …, "source": "nearest" | "implicit" | …}`.
///
/// The reason every read pins it: OpenSpec resolves its root by walking *ancestors*, so a
/// directory with no `openspec/` of its own answers with a parent repository's — and since a live
/// agent's checklist is read from that agent's worktree, the wrong answer is not an error but a
/// plausible board belonging to somebody else.
#[derive(Debug, Clone, Deserialize)]
pub struct Root {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListChanges {
    #[serde(default)]
    pub changes: Vec<ChangeRow>,
    #[serde(default)]
    pub root: Option<Root>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeRow {
    pub name: String,
    #[serde(default)]
    pub completed_tasks: u32,
    #[serde(default)]
    pub total_tasks: u32,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub last_modified: Option<String>,
}

impl From<ChangeRow> for cide_ipc::ChangeSummary {
    fn from(row: ChangeRow) -> Self {
        Self {
            name: cide_ipc::ChangeName(row.name),
            completed_tasks: row.completed_tasks,
            total_tasks: row.total_tasks,
            // Upstream's own word, kept as one. A change whose status this build has not heard of
            // renders as itself rather than vanishing into an `Unknown` arm.
            status: row.status.unwrap_or_else(|| "unknown".to_string()),
            last_modified: row.last_modified,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSpecs {
    #[serde(default)]
    pub specs: Vec<SpecRow>,
    #[serde(default)]
    pub root: Option<Root>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpecRow {
    pub id: String,
    #[serde(default)]
    pub requirement_count: u32,
}

impl From<SpecRow> for cide_ipc::SpecSummary {
    fn from(row: SpecRow) -> Self {
        Self {
            id: cide_ipc::SpecId(row.id),
            requirement_count: row.requirement_count,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShowChange {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub deltas: Vec<DeltaRow>,
    #[serde(default)]
    pub root: Option<Root>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeltaRow {
    #[serde(default)]
    pub spec: String,
    #[serde(default)]
    pub operation: String,
    #[serde(default)]
    pub description: String,
    /// The singular spelling. See [`DeltaRow::requirements`].
    #[serde(default)]
    pub requirement: Option<RequirementRow>,
    /// And the plural one.
    ///
    /// **Both mean the same thing**, and which arrives depends on the delta. They are folded into
    /// one vector on the way out: two spellings of one fact reaching the frontend would be two
    /// code paths in a renderer, and the less-exercised one would rot.
    #[serde(default)]
    pub requirements: Option<Vec<RequirementRow>>,
    #[serde(default)]
    pub rename: Option<RenameRow>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RenameRow {
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequirementRow {
    /// The requirement's prose, **without its `### Requirement:` header** — the CLI folds that
    /// away, which is why the name is recovered from the file by `crate::block::parse` and not
    /// from here. Kept only so the shape parses and the count is available.
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub scenarios: Vec<ScenarioRow>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScenarioRow {
    #[serde(default)]
    pub raw_text: String,
}

impl DeltaRow {
    /// How many requirements this delta carries, from whichever spelling the CLI used.
    ///
    /// **`max`, not a sum.** The CLI sends `requirement` *and* `requirements` — the same
    /// requirement, twice, in two shapes — for a delta with exactly one. Adding them counted
    /// every single-requirement delta twice, which is what the first version of this did: two
    /// identical cards on the panel, and an archive preview that said "2 requirements" about one.
    /// Only the count is taken from here at all; the contents come from the file.
    pub fn requirement_count(&self) -> usize {
        let plural = self.requirements.as_ref().map_or(0, Vec::len);
        let singular = usize::from(self.requirement.is_some());
        plural.max(singular)
    }
}

/// `ADDED` → [`cide_ipc::DeltaOperation::Added`].
///
/// An unknown operation reads as `Added` rather than refusing the whole document: the set is
/// closed upstream, a fifth would be a feature this build predates, and a delta rendered under a
/// slightly wrong heading is a far better failure than a board that will not open. Logged, so the
/// reason a rendering looks odd is findable.
pub fn operation(raw: &str) -> cide_ipc::DeltaOperation {
    match raw.to_ascii_uppercase().as_str() {
        "ADDED" => cide_ipc::DeltaOperation::Added,
        "MODIFIED" => cide_ipc::DeltaOperation::Modified,
        "REMOVED" => cide_ipc::DeltaOperation::Removed,
        "RENAMED" => cide_ipc::DeltaOperation::Renamed,
        other => {
            tracing::debug!(
                operation = other,
                "an openspec delta operation cide does not know"
            );
            cide_ipc::DeltaOperation::Added
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    #[serde(default)]
    pub artifacts: Vec<ArtifactRow>,
    /// `{ "<artifact id>": { outputPath, resolvedOutputPath, existingOutputPaths } }`.
    ///
    /// **The only way a filename is learned.** The artifact set is schema-driven, so
    /// `proposal.md` is a default and not a guarantee; anything that hard-coded it would break on
    /// the first project with a custom schema, and break by reading the wrong file rather than by
    /// failing.
    #[serde(default)]
    pub artifact_paths: std::collections::BTreeMap<String, ArtifactPaths>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRow {
    pub id: String,
    #[serde(default)]
    pub generates: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactPaths {
    #[serde(default)]
    pub output_path: Option<String>,
    #[serde(default)]
    pub resolved_output_path: Option<String>,
    #[serde(default)]
    pub existing_output_paths: Vec<String>,
}

impl Status {
    pub fn into_artifacts(self) -> Vec<cide_ipc::SpecArtifact> {
        self.artifacts
            .into_iter()
            .map(|row| {
                let paths = self
                    .artifact_paths
                    .get(&row.id)
                    .cloned()
                    .unwrap_or_default();
                cide_ipc::SpecArtifact {
                    generates: row
                        .generates
                        .or_else(|| paths.output_path.clone())
                        .unwrap_or_default(),
                    state: artifact_state(row.status.as_deref()),
                    existing: paths
                        .existing_output_paths
                        .iter()
                        .map(PathBuf::from)
                        .collect(),
                    id: row.id,
                }
            })
            .collect()
    }
}

/// An artifact state cide does not know reads as `Ready`.
///
/// The tolerant direction on purpose: `Ready` means "there is work to do here", which is the
/// answer that leaves every button live. Guessing `Done` for an unknown state would grey out the
/// action that fixes it.
fn artifact_state(raw: Option<&str>) -> cide_ipc::ArtifactState {
    match raw.unwrap_or("ready") {
        "done" => cide_ipc::ArtifactState::Done,
        "skipped" => cide_ipc::ArtifactState::Skipped,
        "blocked" => cide_ipc::ArtifactState::Blocked,
        _ => cide_ipc::ArtifactState::Ready,
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyInstructions {
    #[serde(default)]
    pub tasks: Vec<TaskRow>,
    pub progress: ProgressRow,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRow {
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressRow {
    #[serde(default)]
    pub total: u32,
    /// Upstream has spelled this `complete` and `completed` across releases; both are accepted
    /// so a point release cannot silently zero every progress bar.
    #[serde(default, alias = "completed")]
    pub complete: u32,
}

impl From<ApplyInstructions> for cide_ipc::SpecProgress {
    fn from(answer: ApplyInstructions) -> Self {
        Self {
            total: answer.progress.total,
            completed: answer.progress.complete,
            tasks: answer
                .tasks
                .into_iter()
                .map(|task| cide_ipc::SpecTask {
                    done: task.done,
                    description: task.description,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Validation {
    #[serde(default)]
    pub items: Vec<ValidationItem>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationItem {
    #[serde(default)]
    pub valid: bool,
    #[serde(default)]
    pub issues: Vec<IssueRow>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueRow {
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub line: Option<u32>,
    #[serde(default)]
    pub column: Option<u32>,
}

impl From<Validation> for cide_ipc::SpecValidation {
    fn from(answer: Validation) -> Self {
        let issues: Vec<cide_ipc::SpecIssue> = answer
            .items
            .iter()
            .flat_map(|item| item.issues.iter())
            .map(|issue| cide_ipc::SpecIssue {
                level: issue.level.clone().unwrap_or_else(|| "ERROR".to_string()),
                path: issue.path.clone().unwrap_or_default(),
                message: issue.message.clone(),
                line: issue.line,
                column: issue.column,
            })
            .collect();
        Self {
            // Every item, not any item: `--all` validates a whole project and one broken spec
            // makes the answer "not valid". An empty `items` is vacuously valid, which is what a
            // project with nothing to check should report.
            valid: answer.items.iter().all(|item| item.valid),
            issues,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delta_carrying_both_spellings_counts_its_requirement_once() {
        // The real CLI sends **both** `requirement` and `requirements` for a single-requirement
        // delta — the same requirement in two shapes. Summing them was the first version of this
        // and it drew two identical cards for every one-requirement change.
        let both: DeltaRow = serde_json::from_str(&format!(
            "{{\"spec\":\"cap\",\"operation\":\"ADDED\",\"description\":\"d\",\
             \"requirement\":{req},\"requirements\":[{req}]}}",
            req = "{\"text\":\"The app SHALL x.\",\"scenarios\":[]}"
        ))
        .expect("parses");
        assert_eq!(both.requirement_count(), 1);

        let plural_only: DeltaRow = serde_json::from_str(
            "{\"spec\":\"cap\",\"operation\":\"ADDED\",\"description\":\"d\",\
             \"requirements\":[{\"text\":\"a\",\"scenarios\":[]},\
             {\"text\":\"b\",\"scenarios\":[]}]}",
        )
        .expect("parses");
        assert_eq!(plural_only.requirement_count(), 2);

        let neither: DeltaRow = serde_json::from_str(
            "{\"spec\":\"cap\",\"operation\":\"REMOVED\",\"description\":\"d\"}",
        )
        .expect("parses");
        assert_eq!(neither.requirement_count(), 0);
    }

    #[test]
    fn an_operation_this_build_has_not_heard_of_still_renders() {
        assert_eq!(operation("MODIFIED"), cide_ipc::DeltaOperation::Modified);
        assert_eq!(operation("renamed"), cide_ipc::DeltaOperation::Renamed);
        // A fifth operation is a feature this build predates. A delta under a slightly wrong
        // heading beats a board that will not open.
        assert_eq!(operation("DEPRECATED"), cide_ipc::DeltaOperation::Added);
    }

    #[test]
    fn progress_accepts_both_spellings_upstream_has_used() {
        let a: ApplyInstructions =
            serde_json::from_str(r#"{"tasks":[],"progress":{"total":3,"complete":2}}"#).unwrap();
        let b: ApplyInstructions =
            serde_json::from_str(r#"{"tasks":[],"progress":{"total":3,"completed":2}}"#).unwrap();
        let a: cide_ipc::SpecProgress = a.into();
        let b: cide_ipc::SpecProgress = b.into();
        assert_eq!(a.completed, 2);
        assert_eq!(a, b, "a point release must not silently zero every bar");
    }

    #[test]
    fn one_invalid_item_makes_the_whole_answer_invalid() {
        let answer: Validation = serde_json::from_str(
            r#"{"items":[{"valid":true,"issues":[]},
                {"valid":false,"issues":[{"level":"ERROR","message":"no scenarios","line":4}]}]}"#,
        )
        .unwrap();
        let verdict: cide_ipc::SpecValidation = answer.into();
        assert!(!verdict.valid);
        assert_eq!(verdict.issues.len(), 1);
        assert!(verdict.issues[0].blocking());

        // Nothing to check is vacuously valid, which is what an empty project should report.
        let empty: Validation = serde_json::from_str(r#"{"items":[]}"#).unwrap();
        assert!(cide_ipc::SpecValidation::from(empty).valid);
    }

    #[test]
    fn an_artifacts_paths_come_from_the_cli_and_never_from_a_guess() {
        let status: Status = serde_json::from_str(
            r#"{"artifacts":[{"id":"tasks","generates":"tasks.md","status":"ready"}],
                "artifactPaths":{"tasks":{"outputPath":"tasks.md",
                "existingOutputPaths":["/repo/openspec/changes/c/tasks.md"]}}}"#,
        )
        .unwrap();
        let artifacts = status.into_artifacts();
        assert_eq!(artifacts[0].id, "tasks");
        assert_eq!(artifacts[0].state, cide_ipc::ArtifactState::Ready);
        assert_eq!(
            artifacts[0].existing,
            vec![PathBuf::from("/repo/openspec/changes/c/tasks.md")]
        );
    }
}
