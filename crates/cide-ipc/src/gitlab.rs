//! GitLab's durable identities. Remote API data is fetched on demand, never persisted in tabs.
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GitLabAccount {
    pub id: String,
    pub host: String,
    pub username: String,
    #[ts(type = "number")]
    pub user_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GitLabReview {
    pub id: String,
    pub account: String,
    #[ts(type = "number")]
    pub project: u64,
    #[ts(type = "number")]
    pub iid: u64,
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct GitLabPreferences {
    pub exclude_enabled: bool,
    pub excluded_files: Vec<String>,
    /// What "Review with an agent"'s instructions box starts with when no entry of
    /// [`Self::review_prompts`] names the MR's repository. Blank means an empty box, as before.
    pub review_prompt: String,
    /// Per GitLab repository, in the user's order; first non-blank match wins over
    /// [`Self::review_prompt`]. Keyed by the *GitLab* repository rather than the cide project
    /// hosting the run: a review may be hosted by any open project, and what to look for belongs
    /// to the code under review. Matched in `ui/src/gitlab/model.ts::defaultReviewPrompt`.
    /// A list, not a map, so the settings screen keeps the order the user wrote them in.
    pub review_prompts: Vec<GitLabReviewPrompt>,
}
impl Default for GitLabPreferences {
    fn default() -> Self {
        Self {
            exclude_enabled: true,
            excluded_files: vec!["**/*.pb.go".into(), "**/*_grpc.pb.go".into()],
            review_prompt: String::new(),
            review_prompts: Vec::new(),
        }
    }
}

/// One repository's default review instructions. `repository` is `group/repo` or
/// `host/group/repo`, normalised by `cide_gitlab` when saved (no scheme, no slashes at either
/// end, no `.git`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(export)]
pub struct GitLabReviewPrompt {
    pub repository: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GitLabBoard {
    #[ts(type = "number")]
    pub revision: u64,
    pub accounts: Vec<GitLabAccount>,
    pub reviews: Vec<GitLabReview>,
    pub preferences: GitLabPreferences,
}

#[derive(Clone, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
#[ts(export)]
pub enum GitLabRequest {
    Board,
    Comparison {
        document: GitLabDocument,
    },
    Connect {
        host: String,
        token: String,
    },
    Disconnect {
        account: String,
    },
    Preferences {
        preferences: GitLabPreferences,
    },
    List {
        account: String,
        scope: String,
        state: String,
        search: String,
        project: Option<String>,
        page: u32,
    },
    Open {
        account: String,
        url: String,
    },
    Close {
        review: String,
    },
    Detail {
        review: String,
    },
    Diffs {
        review: String,
        #[ts(type = "number")]
        version: u64,
        page: u32,
    },
    Versions {
        review: String,
    },
    Notes {
        review: String,
        page: u32,
    },
    Discussions {
        review: String,
        page: u32,
    },
    Comment {
        review: String,
        body: String,
        discussion: Option<String>,
        #[ts(type = "unknown")]
        position: Option<serde_json::Value>,
    },
    Resolve {
        review: String,
        discussion: String,
        resolved: bool,
    },
    Approve {
        review: String,
        sha: String,
        undo: bool,
    },
    Approvals {
        review: String,
    },
    Pipelines {
        review: String,
        page: u32,
    },
    Jobs {
        review: String,
        #[ts(type = "number")]
        project: u64,
        #[ts(type = "number")]
        pipeline: u64,
        page: u32,
        bridges: bool,
    },
    Trace {
        review: String,
        #[ts(type = "number")]
        project: u64,
        #[ts(type = "number")]
        job: u64,
    },
    File {
        review: String,
        #[ts(type = "number")]
        project: u64,
        path: String,
        sha: String,
    },
    Checkout {
        review: String,
        sha: String,
    },
    SourceTree {
        review: String,
        sha: String,
    },
    SourceFile {
        review: String,
        sha: String,
        path: String,
    },
    AfterPush {
        account: String,
        remote_url: String,
        branch: String,
    },
    /// This review's local drafts, newest last. Never touches GitLab.
    Drafts {
        review: String,
    },
    /// The user's edit of a draft. `None` leaves that half as it is.
    DraftEdit {
        review: String,
        draft: String,
        body: Option<String>,
        severity: Option<GitLabSeverity>,
    },
    DraftDiscard {
        review: String,
        drafts: Vec<String>,
    },
    /// Post each draft as an ordinary thread, one request per draft; a draft leaves the local
    /// store only once its own POST succeeded. Answers a [`GitLabPublished`].
    DraftPublish {
        review: String,
        drafts: Vec<String>,
    },
}

/// How much a review finding matters. Metadata of the draft, shown as a badge beside it — and
/// **never** written into the comment body, so a published comment reads as the reviewer wrote
/// it rather than carrying a label GitLab has no field for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum GitLabSeverity {
    Critical,
    Major,
    Minor,
    Suggestion,
}

impl GitLabSeverity {
    pub const ALL: [GitLabSeverity; 4] = [
        GitLabSeverity::Critical,
        GitLabSeverity::Major,
        GitLabSeverity::Minor,
        GitLabSeverity::Suggestion,
    ];
    pub fn as_str(self) -> &'static str {
        match self {
            GitLabSeverity::Critical => "critical",
            GitLabSeverity::Major => "major",
            GitLabSeverity::Minor => "minor",
            GitLabSeverity::Suggestion => "suggestion",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|s| s.as_str().eq_ignore_ascii_case(text.trim()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum GitLabSide {
    Old,
    New,
}

/// Who wrote a draft: the agent run that proposed it, or the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GitLabDraftAuthor {
    pub label: String,
    pub harness: Option<crate::Harness>,
    /// The run that wrote it, so a run may edit and discard its own drafts and nobody else's.
    pub run: Option<String>,
}

/// A review comment that exists **only in cide** until the user publishes it.
///
/// The GitLab position is computed when the draft is written, against the MR version that was
/// latest then — a line outside every hunk is refused at that moment, while the agent that chose
/// it can still pick another, rather than at publish time when nobody is there to fix it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GitLabDraft {
    pub id: String,
    pub review: String,
    pub severity: GitLabSeverity,
    pub body: String,
    /// `None` for a general comment on the MR.
    pub path: Option<String>,
    pub old_path: Option<String>,
    pub side: Option<GitLabSide>,
    pub line: Option<u32>,
    /// The GitLab text position, SHAs included, ready to POST.
    #[ts(type = "unknown")]
    pub position: Option<serde_json::Value>,
    /// The MR head the position was computed against; a newer head marks the draft outdated.
    pub head_sha: String,
    pub author: GitLabDraftAuthor,
    #[ts(type = "number")]
    pub created_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GitLabPublished {
    pub published: Vec<String>,
    pub failed: Vec<GitLabPublishFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GitLabPublishFailure {
    pub draft: String,
    pub error: String,
}

/// A harness the MR panel can start a review on, and why not when it cannot.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GitLabReviewHarness {
    pub harness: crate::Harness,
    pub unavailable: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GitLabResponse {
    #[ts(type = "unknown")]
    pub data: serde_json::Value,
    pub next_page: Option<u32>,
}

/// Immutable fetch key for a normal workspace review tab. No file contents are persisted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GitLabDocument {
    pub review: String,
    pub path: String,
    pub old_path: String,
    pub base_sha: String,
    pub start_sha: String,
    pub head_sha: String,
    pub mode: GitLabDocumentMode,
    pub new_file: bool,
    pub deleted_file: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum GitLabDocumentMode {
    Diff,
    Source,
    Base,
}
