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

/// One project's servers and store.
pub struct ProjectDiagnostics {
    project: ProjectId,
    roots: Vec<PathBuf>,
    store: Arc<Mutex<DiagnosticStore>>,
    handles: Arc<Mutex<Vec<LspHandle>>>,
    stop: Arc<AtomicBool>,
    pump: Mutex<Option<std::thread::JoinHandle<()>>>,
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

        let Some(server) = server_for(path) else {
            return DefinitionAnswer::Unavailable {
                reason:
                    "cide has no language server for this file type. Rust and Go are supported."
                        .to_string(),
            };
        };

        let requester = {
            let handles = self.handles.lock();
            handles
                .iter()
                .find(|handle| handle.server() == server)
                .map(cide_lsp::LspHandle::requester)
        };
        let Some(requester) = requester else {
            return DefinitionAnswer::Unavailable {
                reason: format!(
                    "{} is not running for this project. Go to definition needs it.",
                    server.binary()
                ),
            };
        };

        // 0-based on the wire, and the column stays UTF-16 — the same conversion `to_lsp` makes,
        // `saturating_sub` included, because a caret at column 1 must not underflow to u32::MAX.
        let params = serde_json::json!({
            "textDocument": { "uri": cide_lsp::convert::path_to_uri(path) },
            "position": {
                "line": line.saturating_sub(1),
                "character": column.saturating_sub(1),
            },
        });

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

    #[test]
    fn the_coalescer_constants_make_a_burst_a_handful_of_emits() {
        // rust-analyzer publishes per file; a workspace check is hundreds in a burst. These
        // numbers are what stand between that and hundreds of serializations on the GTK main
        // loop — the pressure ADR 0003 forced `cide-pty` to coalesce for.
        assert!(COALESCE < COALESCE_CEILING);
        assert!(TICK < COALESCE, "the pump cannot notice its own debounce");
    }
}
