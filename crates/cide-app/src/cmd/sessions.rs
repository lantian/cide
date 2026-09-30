//! The Agents panel's **Sessions** tab: list the journal, search transcripts, open one. (M134)
//!
//! `crate::sessions_state` is the journal and says what a row is; these are its three doors.
//! Every one is `async` and does its work on the blocking pool: the listing stats a transcript per
//! row and reads the head of any that grew, and the search reads whole files — none of which
//! belongs on the GTK loop a synchronous command is polled on.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cide_core::CoreError;
use cide_ipc::sessions::{SessionListing, TranscriptHit, TranscriptSearch};
use cide_ipc::{HarnessSession, ProjectId, RunOpen};
use tauri::{AppHandle, Manager, State};

use crate::agents::AgentRegistry;
use crate::state::SessionRegistry;
use crate::workspace_state::WorkspaceState;

type Result<T> = std::result::Result<T, CoreError>;

/// How many transcripts one search reads before it answers `capped`. The newest first, so what
/// a capped search leaves out is the oldest conversations — the ones least likely to be wanted,
/// and a search the user can narrow with the kind filter.
const SEARCH_FILES: usize = 200;

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| CoreError::Io(format!("the sessions worker did not finish: {error}")))?
}

/// The project's agent sessions, newest first. See `sessions_state::list` for what is hidden.
#[tauri::command(rename_all = "camelCase")]
pub async fn sessions_list(app: AppHandle, project: ProjectId) -> Result<SessionListing> {
    blocking(move || crate::sessions_state::list(&app, project)).await
}

/// The newest search's token. A search whose token is older stops at its next file and answers
/// `Cancelled` — the `git_log` rule: a superseded answer must stop reading the disk, not merely
/// be ignored when it arrives.
static LATEST_SEARCH: AtomicU64 = AtomicU64::new(0);

/// Which of the project's conversations ever said `query`, case-insensitively.
///
/// `token` is the caller's own counter, strictly increasing per window; the backend keeps the
/// largest it has seen. Two windows searching at once cancel each other's older searches, which
/// costs a re-type at worst and is simpler than a token per window nobody needs.
#[tauri::command(rename_all = "camelCase")]
pub async fn sessions_search(
    app: AppHandle,
    project: ProjectId,
    query: String,
    token: u64,
) -> Result<TranscriptSearch> {
    LATEST_SEARCH.fetch_max(token, Ordering::SeqCst);
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Ok(TranscriptSearch::Done {
            hits: Vec::new(),
            capped: false,
        });
    }
    blocking(move || {
        let listing = crate::sessions_state::list(&app, project)?;
        let files: Vec<(String, std::path::PathBuf)> = listing
            .rows
            .into_iter()
            .filter(|row| row.searchable)
            .filter_map(|row| {
                crate::sessions_state::transcript_path(&row.record).map(|p| (row.record.id, p))
            })
            .collect();
        let capped = files.len() > SEARCH_FILES;
        let mut hits = Vec::new();
        for (id, path) in files.into_iter().take(SEARCH_FILES) {
            if LATEST_SEARCH.load(Ordering::SeqCst) != token {
                return Ok(TranscriptSearch::Cancelled);
            }
            let Ok(file) = std::fs::File::open(&path) else {
                continue;
            };
            if let Some(snippet) =
                cide_core::sessions::transcript_find(std::io::BufReader::new(file), &needle)
            {
                hits.push(TranscriptHit { id, snippet });
            }
        }
        Ok(TranscriptSearch::Done { hits, capped })
    })
    .await
}

/// What Open means for one row right now. [`RunOpen`]'s three answers, for `openRun.ts`'s reason:
/// the pane is built on the webview's side, where the spawn plan that decides who owns the child
/// is recorded, so this only says *what* to open.
///
/// The ladder, first answer wins:
///
/// 1. **A run the registry still holds** — its own `open_plan`, which knows about restarts,
///    opencode servers and a review's vanished checkout.
/// 2. **A live child** with no pane showing it (closing a pane never ends a child) — mirror it.
/// 3. **An ended conversation** — continue it in the directory it was started in, unless there
///    is nothing to continue: no id the harness can resume, a directory that is gone (a review's
///    checkout is deleted at quit), or a transcript that is.
#[tauri::command(rename_all = "camelCase")]
pub async fn sessions_open(
    app: AppHandle,
    agents: State<'_, Arc<AgentRegistry>>,
    registry: State<'_, SessionRegistry>,
    project: ProjectId,
    id: String,
) -> Result<RunOpen> {
    let workspace = app
        .try_state::<WorkspaceState>()
        .ok_or_else(|| CoreError::Io("cide is shutting down".into()))?;
    let root = crate::tasks_state::project_root(&workspace, project)?;
    let resume_enabled = workspace.with(|ws| ws.settings.claude.cli.inject.resume.enabled);
    let journal = crate::sessions_state::handle(&app)
        .ok_or_else(|| CoreError::Io("cide is shutting down".into()))?;
    let Some(record) = journal.record(&root, &id) else {
        return Ok(unavailable(
            "That session is no longer in this project's history.",
        ));
    };

    if let Some(run) = record.run
        && agents.knows_run(run)
    {
        return agents.open_plan(project, run, &registry, &root, resume_enabled);
    }

    let conversation = record.known.then(|| HarnessSession {
        harness: record.harness,
        id: record.id.clone(),
        cwd: record.cwd.clone(),
    });
    if let Some(session) = record.session
        && registry.get(session).is_some()
    {
        return Ok(RunOpen::Mirror {
            session,
            continues: conversation,
        });
    }

    let Some(conversation) = conversation else {
        return Ok(unavailable(
            "This session ended before its harness named the conversation, so there is nothing to resume.",
        ));
    };
    if !conversation.cwd.is_dir() {
        return Ok(unavailable(&format!(
            "It was started in {}, which no longer exists — a harness resumes a conversation only from the directory it began in.",
            conversation.cwd.display()
        )));
    }
    let transcript_kept = matches!(
        conversation.harness,
        cide_ipc::Harness::Claude | cide_ipc::Harness::Qwen | cide_ipc::Harness::Codex
    );
    if transcript_kept && crate::sessions_state::transcript_path(&record).is_none() {
        return Ok(unavailable(
            "Its transcript is no longer on disk, so the conversation cannot be resumed.",
        ));
    }
    if conversation.harness == cide_ipc::Harness::Claude && !resume_enabled {
        return Ok(unavailable(
            "Resuming Claude conversations is switched off in Settings.",
        ));
    }
    Ok(RunOpen::Continue { conversation })
}

fn unavailable(reason: &str) -> RunOpen {
    RunOpen::Unavailable {
        reason: reason.to_owned(),
    }
}
