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
//! The exceptions are the ones that really do wait: [`diagnostics_restart`] spawns a process,
//! [`diagnostics_definition`], [`diagnostics_probe`], [`diagnostics_usages`] and
//! [`diagnostics_implementations`] each block on a reply from one, and [`diagnostics_refresh`]
//! stats every path it is about to name. All six go to the blocking pool, because a command
//! polled on the main thread holds the GTK loop and freezes every window in the app.
//!
//! [`diagnostics_usages_cancel`] is deliberately *not* among them: it takes a mutex and pushes one
//! notification, and making it wait would defeat its whole purpose, since the thing it races is a
//! twenty-second wait.

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

/// The shortest a caller may ask this to wait, and the longest.
///
/// The floor exists because a webview that passed `0` would turn every hover into an instant
/// `Timeout` and cache a wrong answer; the ceiling because a `timeoutMs` is a number crossing an
/// IPC boundary and a mistyped one must not park a blocking-pool thread for an hour.
const PROBE_TIMEOUT_FLOOR: std::time::Duration = std::time::Duration::from_millis(100);

/// Which action a Ctrl+click at this position would take: jump, or list usages.
///
/// The discriminator, and — because the Ctrl-hover underline consumes the same answer — the *only*
/// thing that has to be asked to know whether to draw an underline. Deliberately does not search
/// for references: a hover must not be able to start a whole-workspace search.
///
/// `timeoutMs` is optional and clamped to `[100 ms, DEFINITION_TIMEOUT]`. The hover passes a short
/// one and means it: five seconds is right behind a keystroke and wrong behind a pointer, where an
/// underline that arrives five seconds later has been wrong for four of them and the thread is held
/// the whole time. Absent, it is [`DEFINITION_TIMEOUT`], so the click behaves exactly like Go to
/// definition.
///
/// Never `Err`, for the reason [`cide_ipc::ProbeAnswer`] gives.
#[tauri::command(rename_all = "camelCase")]
pub async fn diagnostics_probe(
    registry: State<'_, DiagnosticsRegistry>,
    project: ProjectId,
    path: std::path::PathBuf,
    line: u32,
    column: u32,
    timeout_ms: Option<u32>,
) -> Result<cide_ipc::ProbeAnswer, ()> {
    let Some(diagnostics) = registry.get(project) else {
        return Ok(cide_ipc::ProbeAnswer::Unavailable {
            reason: "No language server is running for this project. Rust needs rust-analyzer \
                     and Go needs gopls on PATH."
                .to_string(),
        });
    };
    let timeout = timeout_ms
        .map(|ms| std::time::Duration::from_millis(u64::from(ms)))
        .unwrap_or(DEFINITION_TIMEOUT)
        .clamp(PROBE_TIMEOUT_FLOOR, DEFINITION_TIMEOUT);
    Ok(tauri::async_runtime::spawn_blocking(move || {
        diagnostics.probe(&path, line, column, timeout)
    })
    .await
    .unwrap_or(cide_ipc::ProbeAnswer::Unavailable {
        reason: "The lookup did not finish.".to_string(),
    }))
}

/// How long Find usages waits before saying so.
///
/// Four times [`DEFINITION_TIMEOUT`], and the difference is the gesture rather than the plumbing.
/// Go to definition is behind a keystroke and five seconds of nothing reads as a freeze; a
/// references search is a *search* — the user has a popup in front of them saying so — and on a
/// widely-used trait method against a cold rust-analyzer it legitimately takes tens of seconds.
/// Finite for the same reason as its neighbour: a server mid-index never answers at all.
const USAGES_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Every place the symbol at this position is used.
///
/// `spawn_blocking` and never `Err`, same as its neighbours. Cancellable through
/// [`diagnostics_usages_cancel`] — which is not decoration: twenty seconds is long enough that the
/// user *will* dismiss the popup first, and without the cancel the server keeps searching a whole
/// workspace for a list nobody will read.
#[tauri::command(rename_all = "camelCase")]
pub async fn diagnostics_usages(
    registry: State<'_, DiagnosticsRegistry>,
    project: ProjectId,
    path: std::path::PathBuf,
    line: u32,
    column: u32,
) -> Result<cide_ipc::UsagesAnswer, ()> {
    let Some(diagnostics) = registry.get(project) else {
        return Ok(cide_ipc::UsagesAnswer::Unavailable {
            reason: "No language server is running for this project. Rust needs rust-analyzer \
                     and Go needs gopls on PATH."
                .to_string(),
        });
    };
    Ok(tauri::async_runtime::spawn_blocking(move || {
        diagnostics.usages(&path, line, column, USAGES_TIMEOUT)
    })
    .await
    .unwrap_or(cide_ipc::UsagesAnswer::Unavailable {
        reason: "The search did not finish.".to_string(),
    }))
}

/// Withdraw the outstanding Find usages. Escape in the popup, and closing it.
///
/// **Not** `spawn_blocking`: it takes a mutex, clones a `Requester` and pushes one notification
/// into a bounded channel — the same argument the module docs make for the document notifications.
/// Making it wait would defeat the point, since the thing it is racing is a twenty-second wait.
///
/// A no-op when nothing is outstanding, which is most calls: the popup cancels on unmount and does
/// not track whether the answer already landed. Being cheap enough not to care is the design.
#[tauri::command(rename_all = "camelCase")]
pub fn diagnostics_usages_cancel(registry: State<'_, DiagnosticsRegistry>, project: ProjectId) {
    if let Some(diagnostics) = registry.get(project) {
        diagnostics.cancel_usages();
    }
}

/// Every place the symbol at this position is *implemented*. (M18)
///
/// The Go half of the M18 report: `textDocument/definition` on a call through an interface
/// resolves to the interface's method — gopls is right, and that is not what the user meant.
/// This asks `textDocument/implementation` instead, which is the protocol's name for the question
/// they were asking.
///
/// Shares [`USAGES_TIMEOUT`] and [`diagnostics_usages_cancel`] with its neighbour above, because
/// it shares the popup: one list on screen, one outstanding request, so Escape cancels whichever
/// is running without the webview having to name it.
///
/// `spawn_blocking` and never `Err`, same as its neighbours.
#[tauri::command(rename_all = "camelCase")]
pub async fn diagnostics_implementations(
    registry: State<'_, DiagnosticsRegistry>,
    project: ProjectId,
    path: std::path::PathBuf,
    line: u32,
    column: u32,
) -> Result<cide_ipc::UsagesAnswer, ()> {
    let Some(diagnostics) = registry.get(project) else {
        return Ok(cide_ipc::UsagesAnswer::Unavailable {
            reason: "No language server is running for this project. Rust needs rust-analyzer \
                     and Go needs gopls on PATH."
                .to_string(),
        });
    };
    Ok(tauri::async_runtime::spawn_blocking(move || {
        diagnostics.implementations(&path, line, column, USAGES_TIMEOUT)
    })
    .await
    .unwrap_or(cide_ipc::UsagesAnswer::Unavailable {
        reason: "The search did not finish.".to_string(),
    }))
}

/// Re-run every analyser for this project, because the user asked. (M18)
///
/// The Problems panel's *Re-run analysis* button and the `problems.refresh` command.
///
/// # Why this returns a sentence
///
/// Because the whole of its effect happens in another process over the following seconds, and a
/// button whose effect is invisible is indistinguishable from a button that is wired to nothing —
/// which is the defect this repository keeps producing and, in this case, the user's own
/// complaint. The caller shows what comes back. It names the servers that were kicked, or says
/// plainly that there was nothing to kick.
///
/// Distinct from [`diagnostics_restart`], and the panel offers both under different labels: this
/// one re-runs the checks and takes seconds, that one replaces the process and, on a large
/// workspace, takes minutes of re-indexing.
///
/// `spawn_blocking`: it takes the handles lock, stats the paths it is about to name, and pushes
/// notifications. None of that waits on a reply, but a `stat` per known path is enough file I/O
/// that it has no business on the thread that owns the GTK loop.
#[tauri::command(rename_all = "camelCase")]
pub async fn diagnostics_refresh(
    app: tauri::AppHandle,
    registry: State<'_, DiagnosticsRegistry>,
    project: ProjectId,
) -> Result<String, String> {
    let Some(diagnostics) = registry.get(project) else {
        return Ok(
            "No language server is running for this project. Rust needs rust-analyzer \
                   and Go needs gopls on PATH."
                .to_string(),
        );
    };
    tauri::async_runtime::spawn_blocking(move || diagnostics.refresh(&app))
        .await
        .map_err(|error| format!("the re-run did not start: {error}"))
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
