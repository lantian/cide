//! Problems: the snapshot the panel reads, and the document notifications that keep a language
//! server's view of a file matching the user's.
//!
//! # Why document sync needs commands at all
//!
//! Rust does not hold the live buffer. `ui/src/store/workspace.ts` is a mirror, the CodeMirror
//! `EditorView` is a DOM instance the webview owns, and `file_write` is the only moment text
//! crosses back. A language server needs the *unsaved* text — that is most of what makes it
//! better than running `cargo check` — so the webview has to send it.
//!
//! [`diagnostics_did_change`] is therefore debounced **on the frontend**, at 300 ms, exactly as
//! `claude_selection_changed` documents for a selection drag: a command per keystroke is an IPC
//! round trip per keystroke.
//!
//! # None of these is `spawn_blocking`, and that is deliberate
//!
//! Each is a `serde` clone and a push into a bounded channel the supervisor thread drains — no
//! file is read, no lock is held for longer than a map lookup. That is the same argument
//! `claude_send_lines` writes out under its own "Not `spawn_blocking`" note.
//!
//! Two exceptions, both of which really do wait: [`diagnostics_restart`] spawns a process, and
//! [`diagnostics_definition`] blocks on a reply from one. Both go to the blocking pool, because a
//! command polled on the main thread holds the GTK loop and freezes every window in the app.

use cide_ipc::{DiagnosticSourceId, DiagnosticsSnapshot, ProjectId};
use tauri::State;

use crate::lsp::DiagnosticsRegistry;
use crate::workspace_state::WorkspaceState;

/// The snapshot as it stands.
///
/// Exists for the same reason `window_awaiting_sessions` does: a window that opens between two
/// `cide://diagnostics` events has heard nothing, and would otherwise paint an empty panel over a
/// project with findings.
///
/// Never `Err`. A project with no entry is `unavailable` with a sentence, because "this project
/// has no language server" is an answer and a rejected promise is not.
#[tauri::command(rename_all = "camelCase")]
pub fn diagnostics_get(
    registry: State<'_, DiagnosticsRegistry>,
    state: State<'_, WorkspaceState>,
    project: ProjectId,
) -> DiagnosticsSnapshot {
    let settings = state.with(|ws| ws.settings.inspections.clone());
    match registry.get(project) {
        Some(diagnostics) => diagnostics.snapshot(&settings),
        None => DiagnosticsSnapshot::Unavailable {
            reason: "No language server is running for this project. Rust needs rust-analyzer \
                     and Go needs gopls on PATH."
                .to_string(),
            sources: Vec::new(),
        },
    }
}

/// The editor opened a file. Tells whichever server owns that language.
#[tauri::command(rename_all = "camelCase")]
pub fn diagnostics_did_open(
    registry: State<'_, DiagnosticsRegistry>,
    project: ProjectId,
    path: std::path::PathBuf,
    version: i32,
    text: String,
) {
    let Some(diagnostics) = registry.get(project) else {
        return;
    };
    diagnostics.notify_document(&path, |session, uri, language_id| {
        session.did_open(uri, language_id, version, text.clone())
    });
}

/// The buffer changed. **Debounced on the frontend** — see the module docs.
#[tauri::command(rename_all = "camelCase")]
pub fn diagnostics_did_change(
    registry: State<'_, DiagnosticsRegistry>,
    project: ProjectId,
    path: std::path::PathBuf,
    version: i32,
    text: String,
) {
    let Some(diagnostics) = registry.get(project) else {
        return;
    };
    // Above this, fall back to save-only sync. `MAX_FILE_BYTES` is 32 MiB, so a 20 MB file every
    // 300 ms is reachable and would be 66 MB/s across the IPC boundary for a file nobody is
    // reading diagnostics on anyway.
    const MAX_SYNC_BYTES: usize = 1 << 20;
    if text.len() > MAX_SYNC_BYTES {
        tracing::debug!(
            path = %path.display(),
            bytes = text.len(),
            "too large for incremental sync; the server sees it on save"
        );
        return;
    }
    diagnostics.notify_document(&path, |session, uri, _| {
        session.did_change(uri, version, text.clone())
    });
}

#[tauri::command(rename_all = "camelCase")]
pub fn diagnostics_did_save(
    registry: State<'_, DiagnosticsRegistry>,
    project: ProjectId,
    path: std::path::PathBuf,
) {
    let Some(diagnostics) = registry.get(project) else {
        return;
    };
    diagnostics.notify_document(&path, |session, uri, _| session.did_save(uri));
}

#[tauri::command(rename_all = "camelCase")]
pub fn diagnostics_did_close(
    registry: State<'_, DiagnosticsRegistry>,
    project: ProjectId,
    path: std::path::PathBuf,
) {
    let Some(diagnostics) = registry.get(project) else {
        return;
    };
    diagnostics.notify_document(&path, |session, uri, _| session.did_close(uri));
}

/// How long Go to Definition waits before saying so.
///
/// At the fast end of this workspace's range on purpose (`cide-pty`'s attach is 2 s,
/// `cide-claude`'s default is 180 s): this one is behind a keystroke, and a user who pressed
/// Ctrl+B needs an answer or an explanation quickly — five seconds of nothing reads as a freeze.
/// Finite because rust-analyzer mid-index legitimately never answers.
const DEFINITION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Where the thing under the caret is declared.
///
/// `async` + `spawn_blocking`, unlike its neighbours above: this one genuinely waits on a reply
/// from another process. A plain `#[tauri::command]` is polled on the main thread and would freeze
/// the GTK loop — the whole window — for up to [`DEFINITION_TIMEOUT`]; `cmd/git.rs` records why an
/// `async fn` whose body never awaits is *not* the fix either.
///
/// Never `Err`. "Still indexing" and "no declaration here" are answers, and each is a different
/// sentence — see [`cide_ipc::DefinitionAnswer`].
#[tauri::command(rename_all = "camelCase")]
pub async fn diagnostics_definition(
    registry: State<'_, DiagnosticsRegistry>,
    project: ProjectId,
    path: std::path::PathBuf,
    line: u32,
    column: u32,
) -> Result<cide_ipc::DefinitionAnswer, ()> {
    // Read out of the managed state *before* the await: a `State<'_, _>` cannot be held across
    // one, the same shape `cmd/session.rs` documents.
    let Some(diagnostics) = registry.get(project) else {
        return Ok(cide_ipc::DefinitionAnswer::Unavailable {
            reason: "No language server is running for this project. Rust needs rust-analyzer \
                     and Go needs gopls on PATH."
                .to_string(),
        });
    };
    Ok(tauri::async_runtime::spawn_blocking(move || {
        diagnostics.definition(&path, line, column, DEFINITION_TIMEOUT)
    })
    .await
    .unwrap_or(cide_ipc::DefinitionAnswer::Unavailable {
        reason: "The lookup did not finish.".to_string(),
    }))
}

/// Restart one analyser after it gave up.
///
/// The panel's only recovery gesture, and the reason the three-crashes rule can be as strict as
/// it is: giving up permanently is tolerable when the user has a way back that is not "restart
/// the application".
#[tauri::command(rename_all = "camelCase")]
pub async fn diagnostics_restart(
    app: tauri::AppHandle,
    registry: State<'_, DiagnosticsRegistry>,
    project: ProjectId,
    source: DiagnosticSourceId,
) -> Result<(), String> {
    let Some(diagnostics) = registry.get(project) else {
        return Ok(());
    };
    // Spawns a process, so unlike its neighbours it goes to the blocking pool.
    tauri::async_runtime::spawn_blocking(move || diagnostics.restart(&app, source))
        .await
        .map_err(|error| format!("the restart did not finish: {error}"))
}
