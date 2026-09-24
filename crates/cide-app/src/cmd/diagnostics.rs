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
//! `EditorPane` debounces its position notes for a scroll: a command per keystroke is an IPC
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
    diagnostics.document_opened(&path, version, text);
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
    diagnostics.document_changed(&path, version, text);
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
    diagnostics.document_closed(&path);
}

/// How long Quick documentation waits before saying so. [`DEFINITION_TIMEOUT`]'s reason, one
/// gesture over: it is behind a keystroke too.
const DOCS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Documentation for what is under the caret. (M60)
///
/// `async` + `spawn_blocking` for [`diagnostics_definition`]'s reason, and never `Err` for its
/// reason too: "still indexing", "no server for this file" and "nothing to say here" are three
/// answers, each with its own sentence — see [`cide_ipc::DocsAnswer`]. `word` is what the editor
/// found under the caret, which a hover page is titled by.
#[tauri::command(rename_all = "camelCase")]
pub async fn docs_lookup(
    registry: State<'_, DiagnosticsRegistry>,
    project: ProjectId,
    path: std::path::PathBuf,
    line: u32,
    column: u32,
    word: String,
) -> Result<cide_ipc::DocsAnswer, ()> {
    let subject = cide_ipc::DocsSubject::Position {
        path,
        line,
        column,
        word,
    };
    docs_for(registry, project, subject).await
}

/// The page a documentation tab is about, asked again. (M60)
///
/// The tab carries a subject and never a page (`cide_ipc::TabKind::Docs`), so the pane asks on
/// every mount — a tab restored from `workspace.json` a week later takes exactly this road. A
/// page the project remembers answers without a round trip; see
/// `ProjectDiagnostics::documentation` for which are remembered.
#[tauri::command(rename_all = "camelCase")]
pub async fn docs_page(
    registry: State<'_, DiagnosticsRegistry>,
    project: ProjectId,
    subject: cide_ipc::DocsSubject,
) -> Result<cide_ipc::DocsAnswer, ()> {
    docs_for(registry, project, subject).await
}

async fn docs_for(
    registry: State<'_, DiagnosticsRegistry>,
    project: ProjectId,
    subject: cide_ipc::DocsSubject,
) -> Result<cide_ipc::DocsAnswer, ()> {
    // Read out of the managed state *before* the await — `diagnostics_definition`'s note.
    let Some(diagnostics) = registry.get(project) else {
        return Ok(cide_ipc::DocsAnswer::Unavailable {
            reason: "No language server is running for this project.".to_string(),
        });
    };
    Ok(tauri::async_runtime::spawn_blocking(move || {
        diagnostics.documentation(&subject, DOCS_TIMEOUT)
    })
    .await
    .unwrap_or(cide_ipc::DocsAnswer::Unavailable {
        reason: "The lookup did not finish.".to_string(),
    }))
}

/// Open a documentation tab, or activate the one already about this subject. (M60)
///
/// `docker_open_inspect`'s shape and its reason: matching on the *subject* rather than the title
/// is what keeps a reference followed twice from minting two identical tabs, and `title` is a
/// caption written from the answer the caller already has.
#[tauri::command(rename_all = "camelCase")]
pub async fn tab_open_docs(
    state: State<'_, WorkspaceState>,
    project: ProjectId,
    subject: cide_ipc::DocsSubject,
    title: String,
) -> std::result::Result<cide_ipc::TabId, cide_core::CoreError> {
    let wanted = subject;
    state.update(|ws| {
        let open = cide_core::workspace::project(ws, project)?;
        let existing = open
            .tabs
            .iter()
            .find(|tab| matches!(&tab.kind, cide_ipc::TabKind::Docs { subject, .. } if *subject == wanted))
            .map(|tab| tab.id);
        if let Some(id) = existing {
            cide_core::workspace::activate_tab(ws, project, id)?;
            return Ok(id);
        }
        let title = if title.trim().is_empty() {
            wanted.label()
        } else {
            title
        };
        cide_core::workspace::open_tab(
            ws,
            project,
            cide_ipc::TabKind::Docs {
                subject: wanted.clone(),
                title,
            },
            cide_ipc::Pane {
                id: cide_ipc::PaneId::new(),
                kind: cide_ipc::PaneKind::Editor,
                role: cide_ipc::PaneRole::Auxiliary,
                // No process — the page renders over the tree, as a Docker inspect page does.
                session: None,
                conversation: None,
                conversation_since: None,
                continues: None,
                harness: None,
                title: "docs".into(),
                docker: None,
            },
        )
    })
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
        let answer = diagnostics.definition(&path, line, column, DEFINITION_TIMEOUT);
        mark_interface_method(answer)
    })
    .await
    .unwrap_or(cide_ipc::DefinitionAnswer::Unavailable {
        reason: "The lookup did not finish.".to_string(),
    }))
}

/// Fill in [`cide_ipc::DefinitionAnswer::Found::interface_method`] by parsing the file the answer
/// points at.
///
/// # Why the target file and not the caret's
///
/// The question is "did this land on an interface method", and only the target can answer it. A
/// call site says nothing: `x.Read(p)` looks identical whether `x` is an interface or a concrete
/// type, which is precisely why the protocol has two requests instead of one.
///
/// # Why here
///
/// `lsp.rs` is a language-server client and has no business parsing Go; `cide_lang` parses Go and
/// has no business knowing what a definition is. This is the seam that already holds both, and it
/// is on the blocking pool, where reading and parsing one file is allowed to cost what it costs.
///
/// Best-effort in every direction. A file that cannot be read — a definition inside `$GOMODCACHE`
/// on a mount that went away, a target the server named but the disk does not have — leaves the
/// flag `false`, and Go to definition does what it did before. The cost is one read and one parse
/// of a file the user is about to open anyway, and only when a definition was actually found.
fn mark_interface_method(answer: cide_ipc::DefinitionAnswer) -> cide_ipc::DefinitionAnswer {
    let cide_ipc::DefinitionAnswer::Found {
        path,
        line,
        column,
        interface_method: _,
    } = answer
    else {
        return answer;
    };

    let interface_method = interface_method_at(&path, line);
    cide_ipc::DefinitionAnswer::Found {
        path,
        line,
        column,
        interface_method,
    }
}

/// Does the declaration at `path:line` belong to an interface?
///
/// The extension gate is a cost decision and not a correctness one:
/// `cide_lang::interface_method_at` already answers `false` for Rust by construction, and its own
/// docs explain why. But this is reached from the **probe**, which the Ctrl-hover underline calls
/// as the pointer moves, and making every hover in a Rust file read and parse the file it points
/// at to be told "no interfaces here" is a cost with a known answer. A Go file pays one read and
/// one tree-sitter parse per distinct target, behind `resolveWord`'s cache.
fn interface_method_at(path: &str, line: u32) -> bool {
    if std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        != Some("go")
    {
        return false;
    }
    std::fs::read_to_string(path).is_ok_and(|text| {
        cide_lang::interface_method_at(
            &cide_lang::outline(
                std::path::Path::new(path),
                &text,
                cide_lang::Limits::default(),
            ),
            line,
        )
    })
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
        // Enriched here and not in `lsp.rs` for the same reason the definition is, and it has to
        // be enriched at all so that Ctrl+click and Ctrl+B do the same thing to the same word.
        // A discriminator the click consults and the keystroke does not is how the two gestures
        // drift apart.
        match diagnostics.probe(&path, line, column, timeout) {
            cide_ipc::ProbeAnswer::Definition {
                path,
                line,
                column,
                interface_method: _,
            } => {
                let interface_method = interface_method_at(&path, line);
                cide_ipc::ProbeAnswer::Definition {
                    path,
                    line,
                    column,
                    interface_method,
                }
            }
            other => other,
        }
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

/// How long a completion waits before giving up. (M25)
///
/// **The shortest deadline in this file, and it is short for the gesture rather than for the
/// plumbing.** Go to definition is five seconds because a user pressed a key and is watching;
/// Find usages is twenty because it draws a popup that says it is searching. A completion popup
/// says nothing and is racing the user's own typing: a list computed for a prefix the user typed
/// two seconds ago is not late, it is *wrong*, and drawing it would move the selection under
/// somebody mid-keystroke. Past this point the honest thing is to have no popup.
///
/// It is also the one deadline that is hit routinely rather than exceptionally — rust-analyzer
/// spends its first minutes indexing, and every keystroke during them ends here. That is
/// precisely why the webview drops an implicit failure in silence.
const COMPLETION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// The completion list at a position. (M25)
///
/// `async` + `spawn_blocking` for [`diagnostics_definition`]'s reason, and it matters more here:
/// this is called on a burst of typing, so a version polled on the main thread would freeze the
/// GTK loop — every window, every terminal — for up to two seconds per keystroke.
///
/// `before` is the character immediately left of the caret, or `None` at the start of a line.
/// A `String` on the wire and not a `char`, because `char` has no JSON representation of its own
/// and one-character strings are what the webview naturally has; only the first character is
/// read, so a caller that sends more is narrowed rather than refused.
///
/// Never `Err`, for the reason [`cide_ipc::CompletionAnswer`] gives — and note that its
/// `Unavailable` is, uniquely in this file, usually shown to nobody.
#[tauri::command(rename_all = "camelCase")]
pub async fn diagnostics_completion(
    registry: State<'_, DiagnosticsRegistry>,
    project: ProjectId,
    path: std::path::PathBuf,
    line: u32,
    column: u32,
    before: Option<String>,
) -> Result<cide_ipc::CompletionAnswer, ()> {
    let Some(diagnostics) = registry.get(project) else {
        return Ok(cide_ipc::CompletionAnswer::Unavailable {
            reason: "No language server is running for this project.".to_string(),
        });
    };
    let before = before.and_then(|text| text.chars().next());
    Ok(tauri::async_runtime::spawn_blocking(move || {
        diagnostics.completion(&path, line, column, before, COMPLETION_TIMEOUT)
    })
    .await
    .unwrap_or(cide_ipc::CompletionAnswer::Unavailable {
        reason: "The completion did not finish.".to_string(),
    }))
}

/// How long a resolve waits. (M25)
///
/// Longer than [`COMPLETION_TIMEOUT`] and for the opposite reason. That deadline is short because
/// the user is still typing and a late list is a wrong list. This one is behind a **Tab the user
/// has already pressed**: they have chosen, nothing is racing it, and the alternative to waiting
/// is refusing an accept. Two seconds would refuse one every time rust-analyzer is busy.
///
/// Still finite, and still shorter than Go to definition's, because the thing being computed is
/// one import path rather than a whole-workspace answer.
const RESOLVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

/// The deferred edits of one completion item, fetched as the user accepts it. (M25)
///
/// `async` + `spawn_blocking` like its neighbours. Called at most once per accepted completion,
/// and only for the rows that said they had something deferred
/// ([`cide_ipc::CompletionItem::resolve`]) — an ordinary accept never reaches here.
///
/// Never `Err`; an `Unavailable` is a sentence and, uniquely among the completion commands, one
/// the user *does* see, because the accept it was blocking has to be refused rather than done by
/// halves.
#[tauri::command(rename_all = "camelCase")]
pub async fn diagnostics_completion_resolve(
    registry: State<'_, DiagnosticsRegistry>,
    project: ProjectId,
    token: u32,
    index: u32,
) -> Result<cide_ipc::CompletionResolveAnswer, ()> {
    let Some(diagnostics) = registry.get(project) else {
        return Ok(cide_ipc::CompletionResolveAnswer::Unavailable {
            reason: "No language server is running for this project.".to_string(),
        });
    };
    Ok(tauri::async_runtime::spawn_blocking(move || {
        diagnostics.resolve_completion(token, index, RESOLVE_TIMEOUT)
    })
    .await
    .unwrap_or(cide_ipc::CompletionResolveAnswer::Unavailable {
        reason: "The lookup did not finish.".to_string(),
    }))
}

/// Withdraw the outstanding completion. The popup closing, and the editor losing its buffer.
///
/// **Not** `spawn_blocking`, for [`diagnostics_usages_cancel`]'s reason. The volume argument is
/// the extra one: this is called whenever a popup closes, which under normal typing is many times
/// a minute, and a blocking-pool hop per close would be a thread handoff to take a mutex.
///
/// A no-op when nothing is outstanding, which is most calls.
#[tauri::command(rename_all = "camelCase")]
pub fn diagnostics_completion_cancel(registry: State<'_, DiagnosticsRegistry>, project: ProjectId) {
    if let Some(diagnostics) = registry.get(project) {
        diagnostics.cancel_completion();
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
/// Stop every analyser, delete every server's on-disk cache, start them all again.
///
/// The nuclear option next to [`diagnostics_restart`]'s per-server one, and the difference is
/// the point: a restart of the bundled rust-analyzer *restores its disk index*, so when the
/// index itself is the problem, restarting reproduces it. The ordering that makes the delete
/// safe lives in `DiagnosticsRegistry::invalidate_caches`, not here.
#[tauri::command(rename_all = "camelCase")]
pub async fn diagnostics_invalidate_caches(app: tauri::AppHandle) -> Result<(), String> {
    // Joins every shutdown ladder and spawns every replacement, so: the blocking pool. The
    // registry is re-fetched from the handle inside the closure because `State` borrows the
    // command's lifetime and the closure needs `'static`.
    tauri::async_runtime::spawn_blocking(move || {
        use tauri::Manager as _;
        let registry = app.state::<DiagnosticsRegistry>();
        registry.invalidate_caches(&app);
    })
    .await
    .map_err(|error| format!("the invalidation did not finish: {error}"))
}

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
