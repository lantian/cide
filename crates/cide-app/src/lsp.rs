//! Language servers, the diagnostics store, and the one thread that joins them.
//!
//! One [`ProjectDiagnostics`] per open project, each owning:
//!
//! * a [`cide_lsp::LspHandle`] per language whose server started,
//! * the merged [`DiagnosticStore`],
//! * a pump thread that drains the handles, folds into the store, and emits — coalesced.
//!
//! # Why the emit is coalesced, and why that is not tuning
//!
//! rust-analyzer publishes **per file**, so a workspace check is hundreds of
//! `publishDiagnostics` in a burst. One `cide://diagnostics` per publish would be hundreds of
//! serializations landing on the GTK main loop, competing with exactly the terminal output ADR
//! 0003 forced `cide-pty` to coalesce for. A 250 ms trailing debounce with a 1 s ceiling turns
//! that burst into four or five emits, and the ceiling means a server that publishes continuously
//! still updates once a second rather than never.
//!
//! # Why the spawn goes through `on_spawn_thread`
//!
//! `cide_core::child_env::arm` requires the forking thread to outlive the child. A Tauri command
//! handler runs on a pooled worker that can retire at any moment, so a server spawned from one
//! would be `SIGTERM`ed a few seconds later for no reason anybody could diagnose. `on_spawn_thread`
//! is the long-lived thread that exists for this.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cide_core::diagnostics::{DiagnosticStore, EMIT_CAP};
use cide_ipc::{DiagnosticsSnapshot, InspectionSettings, ProjectId, SourceStatus};
use cide_lsp::{LspEvent, LspHandle, Server};
use dashmap::DashMap;
use parking_lot::Mutex;

/// Trailing debounce on the emit. See the module docs.
const COALESCE: Duration = Duration::from_millis(250);
/// And the ceiling, so a continuously-publishing server still updates.
const COALESCE_CEILING: Duration = Duration::from_secs(1);
/// How often the pump wakes when nothing is happening.
const TICK: Duration = Duration::from_millis(100);

/// Every project's diagnostics.
#[derive(Default)]
pub struct DiagnosticsRegistry {
    projects: DashMap<ProjectId, Arc<ProjectDiagnostics>>,
}

impl DiagnosticsRegistry {
    pub fn get(&self, project: ProjectId) -> Option<Arc<ProjectDiagnostics>> {
        self.projects.get(&project).map(|entry| entry.clone())
    }

    /// Start whatever servers this project has work for, or record why not.
    ///
    /// Idempotent: a second call for a project that already has an entry returns it. Called from
    /// `project_open` and from the restore path, both of which can name the same project twice.
    pub fn ensure(
        &self,
        app: &tauri::AppHandle,
        project: ProjectId,
        roots: Vec<PathBuf>,
    ) -> Arc<ProjectDiagnostics> {
        if let Some(existing) = self.get(project) {
            return existing;
        }
        let diagnostics = Arc::new(ProjectDiagnostics::start(app.clone(), project, roots));
        self.projects.insert(project, Arc::clone(&diagnostics));
        diagnostics
    }

    /// Stop this project's servers. Dropping the entry runs each handle's shutdown ladder.
    pub fn close(&self, project: ProjectId) {
        self.projects.remove(&project);
    }

    /// Stop every server, for the shutdown ladder.
    pub fn close_all(&self) {
        self.projects.clear();
    }
}

/// The Find usages request that is in flight for this project, if any.
///
/// **One per project, so no id has to cross the IPC boundary.** A second Find usages supersedes the
/// first — the popup only shows one list — and Escape cancels whatever is outstanding, so the
/// webview never needs to name a request. Handing an id to the frontend would mean the frontend
/// holding a number whose only valid use is passing it straight back.
type Outstanding = Mutex<Option<(cide_lsp::Requester, i64)>>;

/// One project's servers and store.
pub struct ProjectDiagnostics {
    project: ProjectId,
    roots: Vec<PathBuf>,
    store: Arc<Mutex<DiagnosticStore>>,
    handles: Arc<Mutex<Vec<LspHandle>>>,
    stop: Arc<AtomicBool>,
    pump: Mutex<Option<std::thread::JoinHandle<()>>>,
    usages_in_flight: Outstanding,
}

impl ProjectDiagnostics {
    fn start(app: tauri::AppHandle, project: ProjectId, roots: Vec<PathBuf>) -> Self {
        let store = Arc::new(Mutex::new(DiagnosticStore::new()));
        let handles: Arc<Mutex<Vec<LspHandle>>> = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));

        for server in [Server::RustAnalyzer, Server::Gopls] {
            // On the spawn thread, not here — see the module docs. `on_spawn_thread` blocks, and
            // a discovery probe plus a `fork` is single-digit milliseconds.
            let roots_for_server = roots.clone();
            let started = cide_core::child_env::on_spawn_thread(move || {
                LspHandle::start(server, roots_for_server)
            });
            match started {
                Ok(handle) => {
                    // Deliberately *not* `Ready`: nothing has answered yet. `Session::new` emits
                    // its own `Scanning` the moment it writes `initialize`, and this is only what
                    // the store says in the gap before that arrives.
                    store.lock().set_status(
                        server.source(),
                        SourceStatus::Scanning {
                            detail: format!("starting {}", server.binary()),
                        },
                    );
                    handles.lock().push(handle);
                }
                Err(error) => {
                    // Not an error the user has to act on unless they wanted that language: "no
                    // Cargo project under this project's roots" is the ordinary answer for a Go
                    // repository. The sentence is carried either way, and the panel decides how
                    // loudly to say it.
                    store.lock().set_status(
                        server.source(),
                        SourceStatus::Unavailable {
                            reason: error.to_string(),
                        },
                    );
                }
            }
        }

        let pump = std::thread::Builder::new()
            .name(format!("cide-diag-{project}"))
            .spawn({
                let store = Arc::clone(&store);
                let handles = Arc::clone(&handles);
                let stop = Arc::clone(&stop);
                let roots = roots.clone();
                move || pump(app, project, roots, store, handles, stop)
            })
            .ok();

        Self {
            project,
            roots,
            store,
            handles,
            stop,
            pump: Mutex::new(pump),
            usages_in_flight: Mutex::new(None),
        }
    }

    pub fn project(&self) -> ProjectId {
        self.project
    }

    /// The snapshot the UI reads, filtered once.
    pub fn snapshot(&self, settings: &InspectionSettings) -> DiagnosticsSnapshot {
        self.store.lock().snapshot(settings, EMIT_CAP)
    }

    /// The store itself, for the MCP tool — which reads it **unfiltered**.
    pub fn store(&self) -> Arc<Mutex<DiagnosticStore>> {
        Arc::clone(&self.store)
    }

    /// Send a document notification to whichever server owns that language.
    ///
    /// A path with no server is silently ignored: opening a `.toml` is not an error, and there is
    /// nothing to tell.
    pub fn notify_document(
        &self,
        path: &std::path::Path,
        message: impl Fn(&cide_lsp::Session, String, &str) -> cide_lsp::Effect,
    ) {
        let Some(server) = server_for(path) else {
            return;
        };
        let uri = cide_lsp::convert::path_to_uri(path);
        for handle in self.handles.lock().iter() {
            if handle.server() != server {
                continue;
            }
            // `Session`'s document builders are pure — they need no session state — so a
            // throwaway one is enough to shape the message. Threading the live session out of
            // the pump thread would mean a lock around the whole protocol conversation.
            let (session, _) = cide_lsp::Session::new(&self.roots, server.binary());
            if let cide_lsp::Effect::Send(value) =
                message(&session, uri.clone(), server.language_id())
            {
                handle.send(value);
            }
        }
    }

    /// Ask the server that owns this file where the thing under the caret is declared.
    ///
    /// **Blocks for up to `timeout`.** Call it from the blocking pool, never from a Tauri command
    /// worker — see `cmd::diagnostics::diagnostics_definition`.
    ///
    /// # The lock is taken to clone a `Requester` and then dropped
    ///
    /// Not held across the wait, and this is the difference between a working feature and a
    /// deadlock: `pump` takes `handles` every 100 ms, and `restart` takes it from a command
    /// thread. Waiting here with the guard alive would stall the project's diagnostics for the
    /// whole timeout and hard-deadlock against a concurrent restart.
    ///
    /// Never returns an error type. Every way this can fail is a sentence the user should read,
    /// which is the rule `diagnostics_get` states one function up.
    pub fn definition(
        &self,
        path: &std::path::Path,
        line: u32,
        column: u32,
        timeout: std::time::Duration,
    ) -> cide_ipc::DefinitionAnswer {
        use cide_ipc::DefinitionAnswer;

        let (server, requester) = match self.requester_for(path) {
            Ok(pair) => pair,
            Err(missing) => {
                return DefinitionAnswer::Unavailable {
                    reason: missing.sentence("Go to definition"),
                };
            }
        };

        let params = position_params(path, line, column);

        match requester.request("textDocument/definition", params, timeout) {
            Ok(value) => match cide_lsp::convert::location(&value) {
                Some((target, line, column)) => DefinitionAnswer::Found {
                    path: target.to_string_lossy().to_string(),
                    line,
                    column,
                },
                // `null`, `[]`, or a `rust-analyzer://` URI with no file behind it. All three mean
                // "nothing to open", and none of them is a malfunction worth alarming anyone with.
                None => DefinitionAnswer::NotFound,
            },
            // Timeout gets its own sentence rather than the generic one: while rust-analyzer is
            // indexing this is the *expected* answer for the first minute of a session, and
            // "no declaration found" would be a lie that teaches the user to stop trying.
            Err(cide_lsp::RequestError::Timeout) => DefinitionAnswer::Unavailable {
                reason: format!(
                    "{} did not answer in time — it is probably still indexing. Try again in a moment.",
                    server.binary()
                ),
            },
            Err(error) => DefinitionAnswer::Unavailable {
                reason: format!("{}: {error}", server.binary()),
            },
        }
    }

    /// Is the caret on a declaration or on a reference? What a Ctrl+click is about to do.
    ///
    /// **Blocks for up to `timeout`.** Same rules as [`Self::definition`]: blocking pool only, and
    /// the handles lock is taken to clone a `Requester` and then dropped.
    ///
    /// # The discriminator, and what it costs when it is wrong
    ///
    /// *"The definition is where I already am."* Ask `textDocument/definition` — the request
    /// Ctrl+click already made — and compare the reply against the position asked about. Same file,
    /// and the caret inside the returned range, means the caret **is** the declaration.
    ///
    /// That is VS Code's rule verbatim (`DefinitionAction` falls through to
    /// `editor.gotoLocation.alternativeDefinitionCommand`, whose default is `goToReferences`), and
    /// it is right for both servers this app ships: rust-analyzer resolves a declaring identifier
    /// to its own position, and gopls maps a declaring ident to its own object position.
    ///
    /// Cost profile, which is the reason it beats the alternatives: **on a reference — the common
    /// case — it is free**, because it is the one request that was already being made. Only on a
    /// declaration is anything extra paid, and there the extra is a definition round trip in front
    /// of a references search that dwarfs it.
    ///
    /// Wrong in each direction, honestly:
    ///
    /// * **"Declaration" when it was a reference.** Needs a server to answer a non-declaration with
    ///   the caret's own position; neither shipped server does that short of a bug. Cost: a usages
    ///   popup instead of a jump. One Escape, nothing moved.
    /// * **"Reference" when it was a declaration.** Real and reachable: `fn fmt` inside
    ///   `impl Display for Foo` resolves to the *trait's* `fn fmt`, so Ctrl+click on it jumps to
    ///   the trait rather than listing implementations. Cost: a jump the user did not ask for —
    ///   which is why the jump goes on the Back stack, and why `navigate.usages` exists as its own
    ///   bindable command that skips this function entirely. That command is not optional; it is
    ///   the only honest answer to "the discriminator guessed wrong".
    ///
    /// # Why this is in Rust and not in the webview
    ///
    /// The containment test needs the reply's `range.end`, and `convert::location` deliberately
    /// throws the end away. Doing it above would mean widening `DefinitionAnswer` with an end that
    /// `goToDefinition` must not believe, and re-implementing path canonicalisation in TypeScript.
    pub fn probe(
        &self,
        path: &std::path::Path,
        line: u32,
        column: u32,
        timeout: std::time::Duration,
    ) -> cide_ipc::ProbeAnswer {
        use cide_ipc::ProbeAnswer;

        let (server, requester) = match self.requester_for(path) {
            Ok(pair) => pair,
            Err(missing) => {
                return ProbeAnswer::Unavailable {
                    reason: missing.sentence("Go to definition"),
                };
            }
        };

        let params = position_params(path, line, column);

        match requester.request("textDocument/definition", params, timeout) {
            Ok(value) => match cide_lsp::convert::first_location(&value) {
                Some(target) => {
                    if same_file(&target.path, path) && contains(&target, line, column) {
                        ProbeAnswer::Declaration
                    } else {
                        ProbeAnswer::Definition {
                            path: target.path.to_string_lossy().to_string(),
                            line: target.line,
                            column: target.column,
                        }
                    }
                }
                // `null`, `[]`, or a `rust-analyzer://` URI with no file behind it — a keyword, a
                // comment, punctuation. Not a malfunction; the hover simply draws nothing.
                None => ProbeAnswer::NotFound,
            },
            // A cancellation is the caller's own doing, so it says nothing the user has to read —
            // but it must not be reported as "no declaration here" either, or a superseded hover
            // would poison the cache with a wrong answer.
            Err(cide_lsp::RequestError::Cancelled) => ProbeAnswer::Unavailable {
                reason: "The lookup was cancelled.".to_string(),
            },
            Err(cide_lsp::RequestError::Timeout) => ProbeAnswer::Unavailable {
                reason: format!(
                    "{} did not answer in time — it is probably still indexing. Try again in a moment.",
                    server.binary()
                ),
            },
            Err(error) => ProbeAnswer::Unavailable {
                reason: format!("{}: {error}", server.binary()),
            },
        }
    }

    /// Every place the symbol at this position is used.
    ///
    /// **Blocks for up to `timeout`**, which for this one is tens of seconds — see
    /// `cmd::diagnostics::USAGES_TIMEOUT`. Blocking pool only, and the handles lock is dropped
    /// before the wait, which matters far more here than on the definition path: holding it would
    /// freeze the project's diagnostics for twenty seconds and hard-deadlock a concurrent restart.
    ///
    /// The request is registered in [`Self::usages_in_flight`] before the wait, so
    /// [`Self::cancel_usages`] can release this thread *and* stop the server. A second call
    /// supersedes the first for the same reason the popup shows one list.
    pub fn usages(
        &self,
        path: &std::path::Path,
        line: u32,
        column: u32,
        timeout: std::time::Duration,
    ) -> cide_ipc::UsagesAnswer {
        use cide_ipc::UsagesAnswer;

        let (server, requester) = match self.requester_for(path) {
            Ok(pair) => pair,
            Err(missing) => {
                return UsagesAnswer::Unavailable {
                    reason: missing.sentence("Find usages"),
                };
            }
        };

        /*
         * The one case worth refusing outright.
         *
         * `Some(false)` means the server answered `initialize` and did not offer
         * `referencesProvider`. Asking anyway would spend the whole deadline and then report
         * "probably still indexing" about a feature that was never going to arrive — precisely
         * the lie `RequestError`'s three variants exist to prevent, one layer up.
         *
         * `None` — the handshake has not finished — is deliberately **not** refused. A Find usages
         * pressed a moment after launch would otherwise be told the server cannot do it, which is
         * the one sentence that is certainly wrong. Ask; let the deadline answer.
         */
        let known = {
            let handles = self.handles.lock();
            handles
                .iter()
                .find(|handle| handle.server() == server)
                .and_then(cide_lsp::LspHandle::supports_references)
        };
        if known == Some(false) {
            return UsagesAnswer::Unavailable {
                reason: format!(
                    "{} does not offer Find usages. It answered the handshake without \
                     `referencesProvider`.",
                    server.binary()
                ),
            };
        }

        let mut params = position_params(path, line, column);
        /*
         * `includeDeclaration: false`, **and** the origin is filtered out of the reply below.
         *
         * Belt and braces on purpose: gopls has historically returned the declaration whatever the
         * flag said, and a list whose first row is "where you already are" is the one row that is
         * certainly useless — the user is looking at it.
         */
        params["context"] = serde_json::json!({ "includeDeclaration": false });

        // Superseding: whatever was outstanding is cancelled before this one is registered. The
        // lock is taken inside `announce` and released there — never held across the wait.
        self.cancel_usages();
        let slot = &self.usages_in_flight;
        let mine = std::cell::Cell::new(0i64);
        let answer = requester.request_tracked("textDocument/references", params, timeout, |id| {
            mine.set(id);
            *slot.lock() = Some((requester.clone(), id));
        });
        /*
         * Clear the slot, but only if it is still ours.
         *
         * A blind `take()` here is the race that matters: a second Find usages started while this
         * one was waiting has already cancelled us, put *its* id in the slot, and is now blocked —
         * and our cleanup would forget it, leaving Escape with nothing to cancel and the server
         * searching the workspace with nobody able to stop it.
         */
        {
            let mut in_flight = slot.lock();
            if in_flight.as_ref().map(|(_, id)| *id) == Some(mine.get()) {
                *in_flight = None;
            }
        }

        match answer {
            Ok(value) => match cide_lsp::convert::locations(&value) {
                Some(rows) => {
                    let (rows, truncated) = self.rows_from(rows, path, line, column);
                    UsagesAnswer::Found { rows, truncated }
                }
                // `null`: there is no symbol at this position at all. Different from an empty
                // list, and the popup says a different sentence for each.
                None => UsagesAnswer::NotFound,
            },
            // Dismissed, or superseded. The caller drops it silently — narrating a key the user
            // just pressed is not a report.
            Err(cide_lsp::RequestError::Cancelled) => UsagesAnswer::Unavailable {
                reason: "The search was cancelled.".to_string(),
            },
            Err(cide_lsp::RequestError::Timeout) => UsagesAnswer::Unavailable {
                reason: format!(
                    "{} did not answer in time — it is probably still indexing. Try again in a moment.",
                    server.binary()
                ),
            },
            Err(error) => UsagesAnswer::Unavailable {
                reason: format!("{}: {error}", server.binary()),
            },
        }
    }

    /// Withdraw the outstanding Find usages, if there is one. Escape, and a superseding request.
    ///
    /// Releases the blocked thread *and* sends `$/cancelRequest`. Both, because "Escape cancelled
    /// it" and "Escape stopped showing it" are indistinguishable on screen and only one of them
    /// would otherwise be true — rust-analyzer would carry on searching a whole workspace for a
    /// popup that is gone.
    pub fn cancel_usages(&self) {
        let outstanding = self.usages_in_flight.lock().take();
        if let Some((requester, id)) = outstanding {
            requester.cancel(id);
        }
    }

    /// Turn LSP locations into rows the popup can draw, reading each file once.
    ///
    /// Capped at [`MAX_USAGES`] rows over [`MAX_USAGE_FILES`] files, because the alternative is
    /// reading four thousand files because somebody Ctrl+clicked `fn new`.
    ///
    /// No `check_within` guard, unlike `cmd::symbols::outline_of`: these paths come from the
    /// *server*, not from the webview, so there is no untrusted input to contain — and Go to
    /// definition has opened `~/.cargo/registry` sources since M12.
    fn rows_from(
        &self,
        locations: Vec<cide_lsp::convert::Loc>,
        origin_path: &std::path::Path,
        origin_line: u32,
        origin_column: u32,
    ) -> (Vec<cide_ipc::Usage>, bool) {
        let mut kept: Vec<cide_lsp::convert::Loc> = Vec::new();
        let mut truncated = false;
        for one in locations {
            // The origin itself, whatever `includeDeclaration` did. See `usages`.
            if same_file(&one.path, origin_path) && contains(&one, origin_line, origin_column) {
                continue;
            }
            if kept.len() >= MAX_USAGES {
                truncated = true;
                break;
            }
            kept.push(one);
        }

        // Grouped by path so each file is read once, and sorted so the popup's order is stable
        // rather than whatever order the server happened to walk its index in.
        kept.sort_by(|a, b| {
            (a.path.as_path(), a.line, a.column).cmp(&(b.path.as_path(), b.line, b.column))
        });

        let mut rows: Vec<cide_ipc::Usage> = Vec::with_capacity(kept.len());
        let mut current: Option<(PathBuf, Vec<String>)> = None;
        let mut files = 0usize;
        for one in kept {
            let lines = match &current {
                Some((path, lines)) if path == &one.path => lines,
                _ => {
                    if files >= MAX_USAGE_FILES {
                        truncated = true;
                        break;
                    }
                    files += 1;
                    // A file that cannot be read yields no text and the rows still ship — a usage
                    // you cannot preview is still a usage, and dropping it would under-report the
                    // count with nothing on screen to say why.
                    let text = std::fs::read_to_string(&one.path).unwrap_or_default();
                    let lines: Vec<String> = text.lines().map(str::to_string).collect();
                    current = Some((one.path.clone(), lines));
                    &current.as_ref().expect("just set").1
                }
            };
            let source = lines
                .get(one.line.saturating_sub(1) as usize)
                .map(String::as_str)
                .unwrap_or("");
            let (text, start, end) = preview(source, one.column, one.end_column);
            rows.push(cide_ipc::Usage {
                rel: relative(&self.roots, &one.path.to_string_lossy()),
                path: one.path.to_string_lossy().to_string(),
                line: one.line,
                column: one.column,
                end_column: one.end_column,
                text,
                start,
                end,
            });
        }
        (rows, truncated)
    }

    /// The server that owns this path and a `Requester` for it, or why there is neither.
    ///
    /// Extracted so the three request paths cannot drift on *which* server owns a file or on the
    /// take-the-lock-then-drop-it rule. The sentence is the caller's, because "Go to definition
    /// needs it" and "Find usages needs it" name different gestures.
    fn requester_for(
        &self,
        path: &std::path::Path,
    ) -> Result<(Server, cide_lsp::Requester), Missing> {
        let Some(server) = server_for(path) else {
            return Err(Missing::NoLanguage);
        };
        // Cloned out from under the lock, and the guard dies at the end of this block. Waiting
        // with it alive would stall the project's diagnostics for the whole timeout and deadlock
        // against a concurrent restart — see the type-level note on `cide_lsp::Requester`.
        let requester = {
            let handles = self.handles.lock();
            handles
                .iter()
                .find(|handle| handle.server() == server)
                .map(cide_lsp::LspHandle::requester)
        };
        match requester {
            Some(requester) => Ok((server, requester)),
            None => Err(Missing::NotRunning(server)),
        }
    }

    /// Restart one source after it gave up.
    pub fn restart(&self, app: &tauri::AppHandle, source: cide_ipc::DiagnosticSourceId) {
        // tree-sitter and Claude are not processes and have nothing to restart.
        let server = match source {
            cide_ipc::DiagnosticSourceId::RustAnalyzer => Server::RustAnalyzer,
            cide_ipc::DiagnosticSourceId::Gopls => Server::Gopls,
            _ => return,
        };
        // Drop the old handle first — its `Drop` runs the ladder — then start a new one on the
        // spawn thread. Doing it the other way round would briefly leave two servers indexing the
        // same workspace, which on rust-analyzer is 2–8 GB.
        self.handles.lock().retain(|h| h.server() != server);
        self.store.lock().clear_source(source);
        let roots = self.roots.clone();
        let started =
            cide_core::child_env::on_spawn_thread(move || LspHandle::start(server, roots));
        match started {
            Ok(handle) => self.handles.lock().push(handle),
            Err(error) => self.store.lock().set_status(
                source,
                SourceStatus::Unavailable {
                    reason: error.to_string(),
                },
            ),
        }
        crate::emit::diagnostics(app, self.project);
    }
}

impl Drop for ProjectDiagnostics {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(pump) = self.pump.lock().take() {
            let _ = pump.join();
        }
        // Each handle's own `Drop` runs its shutdown ladder.
        self.handles.lock().clear();
    }
}

/// Rows a single Find usages will carry. Beyond this the answer says it was truncated.
///
/// Not a politeness limit: without it, Ctrl+clicking `fn new` in a large workspace reads a few
/// thousand files off disk inside one blocking-pool thread to build a list nobody scrolls past the
/// first screen of. Two hundred is well past what anyone reads and far short of what hurts.
const MAX_USAGES: usize = 200;
/// And the files those rows may come from. The second half of the same budget: two hundred usages
/// spread one-per-file is two hundred `read_to_string` calls.
const MAX_USAGE_FILES: usize = 100;

/// Why there is no server to ask.
///
/// A type rather than a string because the *sentence* differs per gesture ("Go to definition needs
/// it" versus "Find usages needs it") while the *condition* does not, and three copies of the
/// condition is how one of them ends up naming the wrong binary.
enum Missing {
    /// Nothing owns this file type. Permanent for this buffer.
    NoLanguage,
    /// The server that owns it is not running for this project. Until a restart.
    NotRunning(Server),
}

impl Missing {
    fn sentence(&self, gesture: &str) -> String {
        match self {
            Self::NoLanguage => {
                "cide has no language server for this file type. Rust and Go are supported."
                    .to_string()
            }
            Self::NotRunning(server) => format!(
                "{} is not running for this project. {gesture} needs it.",
                server.binary()
            ),
        }
    }
}

/// `textDocument` + `position`, in LSP's units.
///
/// 0-based on the wire, and the column stays UTF-16 — the same conversion `to_lsp` makes,
/// `saturating_sub` included, because a caret at column 1 must not underflow to `u32::MAX`.
fn position_params(path: &std::path::Path, line: u32, column: u32) -> serde_json::Value {
    serde_json::json!({
        "textDocument": { "uri": cide_lsp::convert::path_to_uri(path) },
        "position": {
            "line": line.saturating_sub(1),
            "character": column.saturating_sub(1),
        },
    })
}

/// Are these the same file on disk?
///
/// `canonicalize` first, because the two paths reach here by different routes: one is the path the
/// webview holds for the open buffer, the other came back from the server as a `file:` URI it
/// resolved itself. A project opened through a symlink — `~/work/cide` → `/mnt/…` — makes those two
/// spellings of one file, and the discriminator would then call every declaration a reference and
/// Ctrl+click would jump the user to the line they are already on.
///
/// Falls back to a plain comparison when either side cannot be canonicalised, which is the right
/// failure: a file deleted since the server indexed it is not the file under the caret.
fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Is `(line, column)` inside this location's range? Half-open: `[start, end)`.
///
/// Half-open and not closed, and the difference is a real misfire rather than pedantry. Both Ctrl
/// gestures ask about the **start of the word under the pointer**, and a declaration's range starts
/// at exactly that column — so an inclusive start is what makes the common case work. An inclusive
/// *end* would additionally match a range that ends where we asked, i.e. the token immediately
/// before the caret, and call an unrelated identifier's declaration our own.
fn contains(at: &cide_lsp::convert::Loc, line: u32, column: u32) -> bool {
    (line, column) >= (at.line, at.column) && (line, column) < (at.end_line, at.end_column)
}

/// A source line as a usage row carries it: clipped, with the occurrence's byte offsets in it.
///
/// # The units trap, converted once and here
///
/// LSP hands out a **UTF-16 column**; the row renderer wants a **byte offset** into the string it
/// is drawing, because that is `SearchHit`'s convention and `splitHighlight` is shared verbatim
/// with the search panel. The two agree for ASCII and diverge for everything else — one emoji
/// before the identifier moves the highlight two characters, one accented word moves it one — and
/// the divergence is silent. Converting in the webview would mean a second implementation of this
/// against a string that has already been clipped.
///
/// Offsets are clamped into the clipped line rather than trusted: an occurrence past the clip
/// point becomes an empty highlight at the end, which is a row that reads as truncated instead of
/// a `String::slice` that panics inside a render.
fn preview(source: &str, column: u32, end_column: u32) -> (String, u32, u32) {
    /// Byte offset of a 1-based UTF-16 column in `text`, clamped to its length.
    fn byte_of(text: &str, column: u32) -> usize {
        let want = column.saturating_sub(1) as usize;
        let mut units = 0usize;
        for (offset, ch) in text.char_indices() {
            if units >= want {
                return offset;
            }
            units += ch.len_utf16();
        }
        text.len()
    }

    let start = byte_of(source, column);
    let end = byte_of(source, end_column).max(start);
    let clipped = cide_search::content::clip(source, PREVIEW_BYTES);
    let start = start.min(clipped.len());
    let end = end.min(clipped.len());
    (clipped.to_string(), start as u32, end as u32)
}

/// How much of a usage's line is carried.
///
/// `cide_search::content::Limits::max_line_bytes`'s number, deliberately: both surfaces draw a
/// source line in the same row shape, and one of them clipping at a different width would be
/// visible as two different-looking lists of the same file.
const PREVIEW_BYTES: usize = 512;

/// Which server owns a path, by extension.
fn server_for(path: &std::path::Path) -> Option<Server> {
    match cide_lang::Lang::of_path(path)? {
        cide_lang::Lang::Rust => Some(Server::RustAnalyzer),
        cide_lang::Lang::Go => Some(Server::Gopls),
    }
}

/// Drain the handles, fold into the store, emit — coalesced.
fn pump(
    app: tauri::AppHandle,
    project: ProjectId,
    roots: Vec<PathBuf>,
    store: Arc<Mutex<DiagnosticStore>>,
    handles: Arc<Mutex<Vec<LspHandle>>>,
    stop: Arc<AtomicBool>,
) {
    // The two halves of the debounce: when the first un-emitted change arrived, and when the last
    // one did. See the module docs for why both are needed.
    let mut first_dirty: Option<Instant> = None;
    let mut last_change: Option<Instant> = None;

    while !stop.load(Ordering::Acquire) {
        let mut changed = false;
        {
            // The handles lock is held only while draining, never across the emit: `restart`
            // takes it from a command thread, and an emit reaches the webview.
            let handles = handles.lock();
            for handle in handles.iter() {
                let source = handle.server().source();
                for event in handle.drain() {
                    changed = true;
                    match event {
                        LspEvent::Status(status) => store.lock().set_status(source, status),
                        LspEvent::Published { abs_path, items } => {
                            let rel = relative(&roots, &abs_path);
                            let converted = items
                                .iter()
                                .map(|item| {
                                    cide_lsp::convert::diagnostic(
                                        item,
                                        &abs_path,
                                        &rel,
                                        handle.server().binary(),
                                    )
                                })
                                .collect();
                            store.lock().publish(source, abs_path, converted);
                        }
                    }
                }
            }
        }

        let now = Instant::now();
        if changed {
            first_dirty.get_or_insert(now);
            last_change = Some(now);
        }

        let due = match (first_dirty, last_change) {
            (Some(first), Some(last)) => {
                last.elapsed() >= COALESCE || first.elapsed() >= COALESCE_CEILING
            }
            _ => false,
        };
        if due {
            first_dirty = None;
            last_change = None;
            crate::emit::diagnostics(&app, project);
        }

        std::thread::sleep(TICK);
    }
}

/// An absolute path as the panel groups it: relative to its root, forward slashes.
///
/// Falls through to the absolute path when it is under no root, which is a real case —
/// rust-analyzer reports on `~/.cargo/registry` sources when a macro expands into one, and a row
/// the user cannot place is better than a row named by an empty string.
fn relative(roots: &[PathBuf], abs_path: &str) -> String {
    let path = std::path::Path::new(abs_path);
    let multi = roots.len() > 1;
    for root in roots {
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        let rel = rel.to_string_lossy().replace('\\', "/");
        return match (multi, root.file_name()) {
            (true, Some(label)) => format!("{}/{rel}", label.to_string_lossy()),
            _ => rel,
        };
    }
    abs_path.to_string()
}

/// The [`cide_ide_mcp::DiagnosticSource`] the IDE server reads.
///
/// Lives here rather than in `ide.rs` because it is the store's business, and it deliberately
/// holds the store rather than an `AppHandle` — `cide-ide-mcp` must not depend on tauri, and this
/// type is what keeps that true while still answering from live data.
pub struct McpDiagnostics {
    store: Arc<Mutex<DiagnosticStore>>,
}

impl McpDiagnostics {
    pub fn new(store: Arc<Mutex<DiagnosticStore>>) -> Self {
        Self { store }
    }
}

impl cide_ide_mcp::DiagnosticSource for McpDiagnostics {
    fn diagnostics(&self, uri: Option<&str>) -> Vec<cide_ide_mcp::UriDiagnostics> {
        let store = self.store.lock();
        let paths: Vec<String> = match uri {
            Some(uri) => match cide_lsp::convert::uri_to_path(uri) {
                Some(path) => {
                    let path = path.to_string_lossy().into_owned();
                    // `[]` for a file nothing has looked at, `[{uri, diagnostics: []}]` for one
                    // that was checked and is clean. Same rule as the panel's, aimed at an agent.
                    if store.has_looked_at(&path) {
                        vec![path]
                    } else {
                        Vec::new()
                    }
                }
                None => Vec::new(),
            },
            None => store
                .known_paths()
                .into_iter()
                .map(str::to_string)
                .collect(),
        };

        paths
            .into_iter()
            .map(|path| cide_ide_mcp::UriDiagnostics {
                uri: cide_lsp::convert::path_to_uri(std::path::Path::new(&path)),
                diagnostics: store.for_abs_path(&path).into_iter().map(to_lsp).collect(),
            })
            .collect()
    }
}

/// cide's diagnostic back into LSP's shape.
///
/// The inverse of `cide_lsp::convert::diagnostic`, and it has to exist: the CLI reads 0-based
/// positions and a *numeric* severity, and handing it cide's 1-based lines and string severity
/// would be garbage it silently drops.
fn to_lsp(item: &cide_ipc::Diagnostic) -> cide_ide_mcp::LspDiagnostic {
    cide_ide_mcp::LspDiagnostic {
        range: cide_ide_mcp::LspRange {
            start: cide_ide_mcp::LspPosition {
                line: item.line.saturating_sub(1),
                character: item.column.saturating_sub(1),
            },
            end: cide_ide_mcp::LspPosition {
                line: item.end_line.saturating_sub(1),
                character: item.end_column.saturating_sub(1),
            },
        },
        severity: match item.severity {
            cide_ipc::Severity::Error => 1,
            cide_ipc::Severity::Warning => 2,
            cide_ipc::Severity::Info => 3,
            cide_ipc::Severity::Hint => 4,
        },
        code: item.code.clone(),
        source: Some(item.source.clone()),
        message: item.message.clone(),
    }
}

/// Everything a project's diagnostics need, keyed for the registry.
pub type Projects = BTreeMap<ProjectId, Arc<ProjectDiagnostics>>;

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::{DiagnosticKind, DiagnosticSourceId, Severity};

    fn item(line: u32, severity: Severity) -> cide_ipc::Diagnostic {
        cide_ipc::Diagnostic {
            path: "src/a.rs".into(),
            abs_path: "/repo/src/a.rs".into(),
            line,
            column: 5,
            end_line: line,
            end_column: 9,
            severity,
            kind: DiagnosticKind::Semantic,
            message: "boom".into(),
            source: "rust-analyzer".into(),
            code: Some("E0308".into()),
        }
    }

    #[test]
    fn a_diagnostic_converts_back_to_lsps_zero_based_numbering() {
        // The inverse conversion, and the one the CLI reads. Off by one here and every jump the
        // agent makes lands a line early.
        let lsp = to_lsp(&item(12, Severity::Error));
        assert_eq!(lsp.range.start.line, 11);
        assert_eq!(lsp.range.start.character, 4);
        assert_eq!(lsp.severity, 1);
        assert_eq!(lsp.code.as_deref(), Some("E0308"));
    }

    #[test]
    fn every_severity_has_lsps_number_and_not_a_name() {
        for (ours, theirs) in [
            (Severity::Error, 1),
            (Severity::Warning, 2),
            (Severity::Info, 3),
            (Severity::Hint, 4),
        ] {
            assert_eq!(to_lsp(&item(1, ours)).severity, theirs, "{ours:?}");
        }
    }

    #[test]
    fn a_line_one_diagnostic_does_not_underflow_to_the_end_of_the_file() {
        // `saturating_sub`, not `- 1`: a producer that sends a 0 would wrap to `u32::MAX` and put
        // the agent's attention four billion lines down.
        let mut zero = item(1, Severity::Error);
        zero.line = 0;
        zero.column = 0;
        let lsp = to_lsp(&zero);
        assert_eq!(lsp.range.start.line, 0);
        assert_eq!(lsp.range.start.character, 0);
    }

    #[test]
    fn a_path_under_a_root_is_named_the_way_the_panel_groups_it() {
        let roots = vec![PathBuf::from("/repo")];
        assert_eq!(relative(&roots, "/repo/src/a.rs"), "src/a.rs");
    }

    #[test]
    fn a_multi_root_project_prefixes_with_the_root_name() {
        // So one file is named the same here, in the file picker and in the search panel.
        let roots = vec![PathBuf::from("/w/alpha"), PathBuf::from("/w/beta")];
        assert_eq!(relative(&roots, "/w/beta/src/a.rs"), "beta/src/a.rs");
    }

    #[test]
    fn a_path_under_no_root_keeps_its_absolute_name() {
        // Real: rust-analyzer reports on `~/.cargo/registry` sources when a macro expands into
        // one. A row the user cannot place beats a row named by an empty string.
        let roots = vec![PathBuf::from("/repo")];
        assert_eq!(
            relative(&roots, "/home/u/.cargo/registry/src/x.rs"),
            "/home/u/.cargo/registry/src/x.rs"
        );
    }

    #[test]
    fn the_mcp_view_distinguishes_clean_from_never_looked_at() {
        let store = Arc::new(Mutex::new(DiagnosticStore::new()));
        store.lock().publish(
            DiagnosticSourceId::RustAnalyzer,
            "/repo/clean.rs".into(),
            vec![],
        );
        let source = McpDiagnostics::new(Arc::clone(&store));

        use cide_ide_mcp::DiagnosticSource;
        let clean = source.diagnostics(Some("file:///repo/clean.rs"));
        assert_eq!(clean.len(), 1, "a checked file must appear, with no items");
        assert!(clean[0].diagnostics.is_empty());

        let unopened = source.diagnostics(Some("file:///repo/never.rs"));
        assert!(
            unopened.is_empty(),
            "an unopened file must not appear at all"
        );
    }

    #[test]
    fn the_mcp_view_is_not_filtered_by_display_settings() {
        // A user who hid hints made a statement about their own screen, not about what Claude
        // should be told. There is no `InspectionSettings` on this path at all, which is the
        // structural way of guaranteeing it.
        let store = Arc::new(Mutex::new(DiagnosticStore::new()));
        store.lock().publish(
            DiagnosticSourceId::RustAnalyzer,
            "/repo/src/a.rs".into(),
            vec![item(1, Severity::Hint), item(2, Severity::Error)],
        );
        use cide_ide_mcp::DiagnosticSource;
        let files = McpDiagnostics::new(store).diagnostics(None);
        assert_eq!(files[0].diagnostics.len(), 2);
    }

    fn loc(line: u32, column: u32, end_column: u32) -> cide_lsp::convert::Loc {
        cide_lsp::convert::Loc {
            path: PathBuf::from("/repo/src/a.rs"),
            line,
            column,
            end_line: line,
            end_column,
        }
    }

    #[test]
    fn a_caret_on_the_declarations_own_name_is_inside_its_range() {
        // The whole discriminator. Both Ctrl gestures ask about the *start* of the word under the
        // pointer, and a declaration's range starts at exactly that column — so the start has to
        // be inclusive or Ctrl+click on a declaration would jump to the line it is already on.
        assert!(contains(&loc(10, 8, 14), 10, 8), "the first character");
        assert!(contains(&loc(10, 8, 14), 10, 11), "the middle");
    }

    #[test]
    fn the_range_is_half_open_so_the_token_before_the_caret_is_not_ours() {
        // Closed at the end would additionally match a range that *ends* where we asked — the
        // identifier immediately before the caret — and call an unrelated declaration our own.
        assert!(!contains(&loc(10, 8, 14), 10, 14), "one past the end");
        assert!(!contains(&loc(10, 8, 14), 10, 7), "one before the start");
        assert!(!contains(&loc(10, 8, 14), 9, 99), "the line above");
        assert!(!contains(&loc(10, 8, 14), 11, 1), "the line below");
    }

    #[test]
    fn a_multi_line_range_contains_a_column_the_first_line_would_reject() {
        // rust-analyzer returns whole-item ranges for some kinds, so the comparison has to be on
        // the (line, column) pair and not on each axis separately — column 2 on line 11 is inside
        // a range that starts at column 8 on line 10.
        let mut item = loc(10, 8, 4);
        item.end_line = 12;
        assert!(
            contains(&item, 11, 2),
            "a column on a middle line must not be compared against the first line's column"
        );
        assert!(!contains(&item, 12, 4), "still half-open at the end");
    }

    #[test]
    fn a_preview_reports_byte_offsets_and_not_utf16_columns() {
        // The silent one. LSP counts UTF-16 units, `splitHighlight` slices bytes, and the two
        // agree only for ASCII — one emoji before the identifier moves the highlight by two.
        let source = "let 🦀 = target();";
        // `target` starts at UTF-16 column 10: `let ` (4) + the crab (2 units) + ` = ` (3) = 9.
        let (text, start, end) = preview(source, 10, 16);
        assert_eq!(text, source);
        assert_eq!(&text[start as usize..end as usize], "target");
    }

    #[test]
    fn an_ascii_preview_is_the_columns_minus_one() {
        let (text, start, end) = preview("    target()", 5, 11);
        assert_eq!((start, end), (4, 10));
        assert_eq!(&text[start as usize..end as usize], "target");
    }

    #[test]
    fn a_preview_of_a_line_that_could_not_be_read_is_empty_rather_than_absent() {
        // The row still ships — a usage you cannot preview is still a usage. Empty text with
        // clamped offsets, never a panic inside a render.
        let (text, start, end) = preview("", 5, 11);
        assert_eq!(text, "");
        assert_eq!((start, end), (0, 0));
    }

    #[test]
    fn an_occurrence_past_the_clip_point_clamps_instead_of_panicking() {
        // A minified bundle: one line of megabytes with the identifier three hundred thousand
        // bytes in. The row reads as truncated; the alternative is a `String::slice` out of range
        // inside a render, which unmounts the popup.
        let source = format!("{}target", "x".repeat(PREVIEW_BYTES * 2));
        let (text, start, end) = preview(
            &source,
            PREVIEW_BYTES as u32 * 2 + 1,
            PREVIEW_BYTES as u32 * 2 + 7,
        );
        assert_eq!(text.len(), PREVIEW_BYTES);
        assert_eq!((start, end), (PREVIEW_BYTES as u32, PREVIEW_BYTES as u32));
    }

    #[test]
    fn a_preview_never_cuts_a_character_in_half() {
        // `clip` backs up to a boundary; without that the `String::from_utf8` behind
        // `to_string()` would be operating on a partial code point.
        let source = format!("{}日本語", "x".repeat(PREVIEW_BYTES - 1));
        let (text, _, _) = preview(&source, 1, 2);
        assert!(text.len() <= PREVIEW_BYTES);
        assert!(source.starts_with(&text));
    }

    #[test]
    fn each_gesture_names_itself_in_the_no_server_sentence() {
        // One condition, two sentences. Three copies of the condition is how one of them ends up
        // naming the wrong binary.
        let missing = Missing::NotRunning(Server::Gopls);
        assert!(
            missing
                .sentence("Find usages")
                .contains("gopls is not running")
        );
        assert!(
            missing
                .sentence("Find usages")
                .contains("Find usages needs it")
        );
        assert!(
            missing
                .sentence("Go to definition")
                .contains("Go to definition needs it")
        );
        // And the permanent one says what *is* supported rather than naming a binary that would
        // not help.
        assert!(
            Missing::NoLanguage
                .sentence("Find usages")
                .contains("Rust and Go are supported")
        );
    }

    #[test]
    fn a_file_is_the_same_file_as_itself_by_spelling_alone() {
        // The fast path, and the only one a unit test can assert without touching the disk:
        // `canonicalize` needs a real file, so the symlink case is covered by the fallback below
        // rather than by a fixture.
        assert!(
            same_file(
                std::path::Path::new("/repo/src/a.rs"),
                std::path::Path::new("/repo/src/a.rs")
            ),
            "one spelling of one path is one file"
        );
        // Two paths that do not exist and do not match are not the same file. `canonicalize` fails
        // on both, and the fallback must be `false` — a `true` here would make the discriminator
        // call every definition a declaration.
        assert!(
            !same_file(
                std::path::Path::new("/repo/src/a.rs"),
                std::path::Path::new("/repo/src/b.rs")
            ),
            "two paths that neither match nor canonicalise are not the same file — a `true` here \
             makes the discriminator call every definition a declaration"
        );
    }

    #[test]
    fn the_usage_caps_are_a_budget_and_not_a_politeness() {
        // Both halves of it: two hundred usages spread one per file is two hundred file reads, so
        // capping only the rows would leave the expensive axis unbounded.
        // `const` blocks, because clippy is right that a comparison of two literals is decided at
        // compile time — which is the point: this is a guard against somebody editing one of the
        // two numbers, and it should fail the build rather than a test run.
        const { assert!(MAX_USAGES > 0 && MAX_USAGE_FILES > 0) };
        const { assert!(MAX_USAGE_FILES <= MAX_USAGES) };
        // The same clip width the search panel uses, so one file does not read as two different
        // lists in two surfaces.
        assert_eq!(
            PREVIEW_BYTES,
            cide_search::content::Limits::default().max_line_bytes
        );
    }

    #[test]
    fn the_coalescer_constants_make_a_burst_a_handful_of_emits() {
        // rust-analyzer publishes per file; a workspace check is hundreds in a burst. These
        // numbers are what stand between that and hundreds of serializations on the GTK main
        // loop — the pressure ADR 0003 forced `cide-pty` to coalesce for.
        assert!(COALESCE < COALESCE_CEILING);
        assert!(TICK < COALESCE, "the pump cannot notice its own debounce");
    }
}
