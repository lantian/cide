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
}
impl Default for GitLabPreferences {
    fn default() -> Self {
        Self {
            exclude_enabled: true,
            excluded_files: vec!["**/*.pb.go".into(), "**/*_grpc.pb.go".into()],
        }
    }
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
