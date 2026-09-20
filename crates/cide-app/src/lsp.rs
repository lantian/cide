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
//!
//! # How a write nobody typed reaches a language server (M18)
//!
//! cide's premise is that an agent edits the tree while the user watches, so the interesting file
//! change is the one the editor never saw. Until M18 nothing carried it to a server:
//! `files::on_watch_event` updated the index, the picker and `cide-lang`, and stopped. The result
//! was the user's report — Claude fixes a hint, the row stays, and clicking it lands in a comment,
//! because the row still holds the line number it had when it was published.
//!
//! The path now is:
//!
//! ```text
//! notify → cide_fs::watch → files::on_watch_event → FsEvents::files_changed
//!        → ProjectDiagnostics::files_changed
//!            ├─ DiagnosticStore::mark_dirty   → the panel says "may be out of date"
//!            ├─ workspace/didChangeWatchedFiles → gopls, at once
//!            └─ rust-analyzer/runFlycheck       → debounced, see `KICK_*`
//! ```
//!
//! The two servers need different things and the difference is not cosmetic:
//! `cide_lsp::session::declares_watched_files` carries that argument. The debounce on the flycheck
//! kick is a correctness requirement rather than tuning, exactly as the emit coalescer below is: a
//! `cargo fmt` over four hundred files must produce **one** `cargo check`, not four hundred.

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
use tauri::Manager;

/// Trailing debounce on the emit. See the module docs.
const COALESCE: Duration = Duration::from_millis(250);
/// And the ceiling, so a continuously-publishing server still updates.
const COALESCE_CEILING: Duration = Duration::from_secs(1);
/// How often the pump wakes when nothing is happening.
const TICK: Duration = Duration::from_millis(100);

/// Trailing debounce on the *flycheck kick* — the `cargo check` re-run an on-disk write asks for.
///
/// Longer than [`COALESCE`], and the difference is what each one costs. Coalescing an emit that
/// fired too early costs a redundant serialization; kicking flycheck too early costs a whole
/// `cargo check` of the workspace, and a `cargo fmt`, a `git checkout` or a branch switch is
/// hundreds of watcher events inside a second. The same argument the coalescer's own comment
/// makes, one order of magnitude up.
const KICK_DEBOUNCE: Duration = Duration::from_millis(500);
/// And the ceiling, so a build that writes continuously still gets re-checked rather than never.
///
/// Without it, `cargo watch` in a terminal pane — or any tool that touches a file every few
/// hundred milliseconds — would reset the trailing debounce for ever and the kick would never
/// fire, which is the failure mode a bare trailing debounce always has.
const KICK_CEILING: Duration = Duration::from_secs(5);

/// Every project's diagnostics.
#[derive(Default)]
pub struct DiagnosticsRegistry {
    projects: DashMap<ProjectId, Arc<ProjectDiagnostics>>,
}

impl DiagnosticsRegistry {
    pub fn get(&self, project: ProjectId) -> Option<Arc<ProjectDiagnostics>> {
        self.projects.get(&project).map(|entry| entry.clone())
    }

    /// Bring every open project's servers into line with the registry. (M22)
    ///
    /// Called when the enabled extension set changes — see `ext_state::publish`. Every project,
    /// because an extension is global: it is not installed *for* a project, so a set that changed
    /// while three are open has changed for all three.
    pub fn sync_all(&self, app: &tauri::AppHandle) {
        // Collected before iterating, and not held across the calls: `sync` spawns on another
        // thread and can take milliseconds, and a `DashMap` iterator held over that blocks every
        // `ensure` and `close` behind it.
        let projects: Vec<Arc<ProjectDiagnostics>> =
            self.projects.iter().map(|entry| entry.clone()).collect();
        for project in projects {
            project.sync(app);
        }
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

    /// Restart one source in every open project. (M25)
    ///
    /// The reaction to the Settings row that flips a server between the bundled build and the
    /// system one: the choice is global — it is not stored per project — so a change means
    /// every project running that server is running the wrong binary. The restart itself is
    /// [`ProjectDiagnostics::restart`], the same road the panel's Restart button takes, which
    /// is what makes the settings row cost nothing new in mechanism.
    pub fn restart_everywhere(&self, app: &tauri::AppHandle, source: cide_ipc::DiagnosticSourceId) {
        // Collected first for `sync_all`'s reason: `restart` blocks on the spawn thread, and a
        // `DashMap` iterator held over that blocks `ensure` and `close` behind it.
        let projects: Vec<Arc<ProjectDiagnostics>> =
            self.projects.iter().map(|entry| entry.clone()).collect();
        for project in projects {
            project.restart(app, source.clone());
        }
    }

    /// Stop every server, delete every server's on-disk cache, start them again. (M25)
    ///
    /// The palette's *Invalidate index caches and restart* — the way back when a disk index
    /// has gone wrong in a way a plain restart cannot fix, because a restart *restores the
    /// snapshot*, which is the thing the user is trying to be rid of.
    ///
    /// Order is load-bearing twice over. Every handle must be **dead** before anything is
    /// deleted: the bundled rust-analyzer saves its index on shutdown, so a delete racing a
    /// dying server gets the snapshot re-created behind it — and `LspHandle`'s `Drop` joins
    /// the whole shutdown ladder, so `stop_servers` returning is the proof of death. And
    /// every project must have stopped before anything restarts, because the cache
    /// directory is **per server, not per project**: a survivor in another project would
    /// re-save what was just deleted, and a restarted server would restore it.
    pub fn invalidate_caches(&self, app: &tauri::AppHandle) {
        // Collected first for `sync_all`'s reason: the ladders and the respawns block, and a
        // `DashMap` iterator held over that blocks `ensure` and `close` behind it.
        let projects: Vec<Arc<ProjectDiagnostics>> =
            self.projects.iter().map(|entry| entry.clone()).collect();
        for project in &projects {
            project.stop_servers();
        }
        for server in cide_lsp::discover::servers() {
            // One leaf per server under the profile's cache dir — the same naming rule
            // `cide_lsp::config` uses when it hands the directory to the server. Deleting a
            // dir that was never created is a fine answer; anything else is worth a line in
            // the log, because a half-deleted index is exactly what this command exists to
            // never leave behind.
            let dir = cide_core::persist::cache_dir().join(server.binary());
            if let Err(error) = std::fs::remove_dir_all(&dir)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(%error, dir = %dir.display(), "invalidate: cache dir not fully removed");
            }
        }
        for project in &projects {
            project.sync(app);
        }
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

/// The Find usages *or* Go to implementation request in flight for this project, if any. (M18)
///
/// **One per project, so no id has to cross the IPC boundary.** A second search supersedes the
/// first — the popup only shows one list — and Escape cancels whatever is outstanding, so the
/// webview never needs to name a request. Handing an id to the frontend would mean the frontend
/// holding a number whose only valid use is passing it straight back.
///
/// Shared between the two gestures rather than given one slot each, and that follows from the same
/// fact: they share the popup. Two slots would mean an Escape that cancelled one kind of search
/// and left the other running, with the user looking at a dismissed popup and rust-analyzer still
/// working — the exact half-cancellation `cancel_usages` was written to prevent.
type Outstanding = Mutex<Option<(cide_lsp::Requester, i64)>>;

/// One completion reply's originals, kept until a newer one displaces it.
/// See [`ProjectDiagnostics::completion_cache`].
struct CompletionReplyCache {
    token: u32,
    server: Server,
    /// Only the rows that had something deferred — `convert::CompletionReply::resolvable`.
    items: Vec<serde_json::Value>,
}

/// How many replies' originals are held. See the field for why it is not one.
const COMPLETION_CACHE_DEPTH: usize = 2;

/// What the pump still owes the servers and the panel after a disk change. (M18)
///
/// # Why the work is parked here rather than done where it is discovered
///
/// [`ProjectDiagnostics::files_changed`] runs on the **watcher thread**, which also carries every
/// file-tree update for the project. Two things must not happen there: a `cargo check` must not be
/// started per watcher event (a `cargo fmt` over four hundred files is four hundred events inside
/// a second), and an emit must not be pushed per event either. So the discovery side records
/// *that* something is owed and the pump — which already wakes every [`TICK`] and already owns the
/// only coalescer in this file — decides when to pay it.
///
/// A `Mutex` and not atomics: `first` and `last` have to move together or the ceiling and the
/// trailing debounce disagree about the same burst.
#[derive(Default)]
struct Kick {
    state: Mutex<KickState>,
}

#[derive(Default)]
struct KickState {
    /// When the first un-serviced change of this burst arrived. Drives [`KICK_CEILING`].
    first: Option<Instant>,
    /// And the most recent. Drives [`KICK_DEBOUNCE`].
    last: Option<Instant>,
    /// The store's staleness marks moved, so the snapshot has changed even though no server spoke.
    ///
    /// Separate from the flycheck timers because it is due *immediately* — the whole point of the
    /// stale flag is that the user sees it while the re-check is still pending — and it rides the
    /// emit coalescer rather than the kick debounce.
    marked: bool,
}

impl Kick {
    /// Record that a disk change arrived, and whether it moved the panel's staleness marks.
    fn schedule(&self, marked: bool) {
        let now = Instant::now();
        let mut state = self.state.lock();
        state.first.get_or_insert(now);
        state.last = Some(now);
        state.marked |= marked;
    }

    /// Has the panel got something new to say? Consumes the flag.
    fn take_marked(&self) -> bool {
        std::mem::take(&mut self.state.lock().marked)
    }

    /// Is the burst over (or has it run long enough)? Consumes the timers when it is.
    fn take_due(&self) -> bool {
        let mut state = self.state.lock();
        let (Some(first), Some(last)) = (state.first, state.last) else {
            return false;
        };
        if last.elapsed() < KICK_DEBOUNCE && first.elapsed() < KICK_CEILING {
            return false;
        }
        state.first = None;
        state.last = None;
        true
    }

    /// Forget whatever was owed — a manual re-run has just paid it in full.
    fn clear(&self) {
        *self.state.lock() = KickState::default();
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
    usages_in_flight: Outstanding,
    /// The completion request in flight, if any — **its own slot, never `usages_in_flight`**.
    ///
    /// Sharing one slot is what `usages` and `implementations` deliberately do, because they
    /// share one popup and starting either genuinely means abandoning the other. Completion
    /// shares nothing with them: it fires on a keystroke, many times a second, and folding it in
    /// would mean every character typed cancels a Find usages the user is waiting on. The
    /// superseding *within* completion is still wanted — a newer keystroke's list is the only one
    /// anybody will read — which is why it is a slot at all rather than nothing.
    completion_in_flight: Outstanding,
    /// The raw items of the last few completion replies, so an accepted row can be resolved.
    ///
    /// # Why a cache rather than a round trip through the webview
    ///
    /// `completionItem/resolve` takes the original item back, `data` and all. The webview could
    /// carry it — an opaque string in the DTO, handed back on accept — and that is a real design
    /// with no staleness in it at all. It was rejected on payload: the converted items measure
    /// ~44 KB for a 118-row reply, and shipping the originals beside them roughly doubles that,
    /// several times a second, while somebody types.
    ///
    /// # Why **two** replies and not one
    ///
    /// One is very nearly enough and fails in a way nobody would find. CodeMirror keeps showing
    /// the list it has while a newer query is in flight, so there is a window — between a newer
    /// reply landing here and CodeMirror swapping its options — where a Tab accepts a row from
    /// the *previous* reply. With one slot that row's handle is already gone and its import is
    /// silently not added. Holding the previous reply as well closes the window, and the cost is
    /// one more list of the minority of rows that had anything deferred at all.
    ///
    /// Newest last. `Server` rides along because the resolve must go back to the server that
    /// answered, and by then the path is not in the caller's hands.
    completion_cache: Mutex<Vec<CompletionReplyCache>>,
    /// Names each reply, so a resolve can say which one it means. Monotonic, never reused.
    completion_token: std::sync::atomic::AtomicU32,
    /// What the pump owes after a disk change. See [`Kick`].
    kick: Arc<Kick>,
    /// The buffers the editor has told this project's servers about. See [`OpenDocuments`].
    open_docs: OpenDocuments,
    /// Documentation pages by the subject that names them, newest last. See
    /// [`Self::documentation`] for what is remembered and why.
    docs_cache: Mutex<Vec<(cide_ipc::DocsSubject, cide_ipc::SymbolDocs)>>,
}

/// How many documentation pages a project remembers. A page is a screenful; this is a few
/// hundred kilobytes at most, and the reason for remembering any is one round trip saved on the
/// tab that opens on an answer — see [`ProjectDiagnostics::documentation`].
const DOCS_CACHE_DEPTH: usize = 32;

/// One open buffer, as the last `didOpen` or `didChange` described it. (M59)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenDocument {
    pub version: i32,
    pub text: String,
}

/// The buffers the editor has told this project's servers about, by path. (M59)
///
/// A server's life ends — a crash-restart, the memory watchdog, the Restart button, or the Godot
/// editor closing under an attached session — and its successor knows nothing of the documents
/// the previous life was told about. `ui/src/editor/docSync.ts` refcounts opens per path and
/// sends each `didOpen` exactly once; it listens to no status and cannot know a life ended. So
/// the app keeps the last full text (sync is full-text, so the copy is exact) and replays a
/// `didOpen` per document on [`LspEvent::Handshook`]. rust-analyzer and gopls read the disk and
/// hid the gap from M3 to M59; Godot publishes only on open, change and save, and a reconnect
/// without this sat silent until the user typed.
///
/// A `didChange` the frontend skipped as too large keeps the `didOpen` text here, which is what
/// the server has anyway.
type OpenDocuments = Arc<Mutex<BTreeMap<PathBuf, OpenDocument>>>;

/// The open documents a freshly handshaken server should be told about: the ones it owns.
///
/// Pure over the ownership question so it can be tested without the process-global registry.
fn owned_documents(
    docs: &BTreeMap<PathBuf, OpenDocument>,
    owns: impl Fn(&std::path::Path) -> bool,
) -> Vec<(&PathBuf, &OpenDocument)> {
    docs.iter().filter(|(path, _)| owns(path)).collect()
}

impl ProjectDiagnostics {
    fn start(app: tauri::AppHandle, project: ProjectId, roots: Vec<PathBuf>) -> Self {
        let store = Arc::new(Mutex::new(DiagnosticStore::new()));
        let handles: Arc<Mutex<Vec<LspHandle>>> = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));

        // Every server in the registry, not the two builtins by name. (M22)
        //
        // The registry is installed from `ExtStore`'s resolved set before the first window exists,
        // so by the time a project opens it holds rust-analyzer, gopls and whatever the enabled
        // extensions declared. `sync_servers` is the same walk, and it runs again whenever that
        // set changes — see its own note for why an already-open project must not be left behind.
        sync_servers(
            &store,
            &handles,
            &roots,
            &binary_choices(&app),
            server_tuning(&app),
        );

        let kick = Arc::new(Kick::default());
        let open_docs: OpenDocuments = Arc::new(Mutex::new(BTreeMap::new()));
        let pump = std::thread::Builder::new()
            .name(format!("cide-diag-{project}"))
            .spawn({
                let store = Arc::clone(&store);
                let handles = Arc::clone(&handles);
                let stop = Arc::clone(&stop);
                let kick = Arc::clone(&kick);
                let open_docs = Arc::clone(&open_docs);
                let roots = roots.clone();
                move || pump(app, project, roots, store, handles, stop, kick, open_docs)
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
            completion_in_flight: Mutex::new(None),
            completion_cache: Mutex::new(Vec::new()),
            completion_token: std::sync::atomic::AtomicU32::new(0),
            kick,
            open_docs,
            docs_cache: Mutex::new(Vec::new()),
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

    /// The editor opened a file: remember it, and tell whichever server owns it. (M59)
    ///
    /// The three `document_*` methods are the only writers of [`OpenDocuments`], and every path
    /// that tells a server about a buffer goes through one of them — a `didOpen` sent around
    /// them would be a document the next life of that server is never told about.
    pub fn document_opened(&self, path: &std::path::Path, version: i32, text: String) {
        self.open_docs.lock().insert(
            path.to_path_buf(),
            OpenDocument {
                version,
                text: text.clone(),
            },
        );
        self.notify_document(path, |session, uri, language_id| {
            session.did_open(uri, language_id, version, text.clone())
        });
    }

    /// The buffer changed. A change for a path never opened updates nothing here and is still
    /// sent, which is the pre-M59 behaviour exactly.
    pub fn document_changed(&self, path: &std::path::Path, version: i32, text: String) {
        if let Some(doc) = self.open_docs.lock().get_mut(path) {
            doc.version = version;
            doc.text = text.clone();
        }
        self.notify_document(path, |session, uri, _| {
            session.did_change(uri, version, text.clone())
        });
    }

    /// The editor closed the file: forget it, and tell the server.
    pub fn document_closed(&self, path: &std::path::Path) {
        self.open_docs.lock().remove(path);
        self.notify_document(path, |session, uri, _| session.did_close(uri));
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
        // Resolved here rather than through `server_for`, which finds the same server and
        // discards the language on the way — and the language is needed twice: `by_language`
        // routes to the server, and `language_id_for` labels the document. Not
        // `server.language_id()`: the didOpen label picks the dialect parser inside a
        // multi-language server, and a css server handed an SCSS document labelled `css`
        // reports every nested rule as an error.
        let Some(language) = language_of(path) else {
            return;
        };
        let Some(server) = cide_lsp::discover::by_language(&language) else {
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
            let (session, _) = cide_lsp::Session::new(&self.roots, server);
            if let cide_lsp::Effect::Send(value) = message(
                &session,
                uri.clone(),
                &server.language_id_for(Some(&language)),
            ) {
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
                    // Not decided here. This layer holds a language-server client and knows
                    // nothing about Go's grammar; the answer comes from parsing the *target*
                    // file, which is `cmd::diagnostics`'s job because that is where the outline
                    // is reachable. `false` is the honest default for a caller that does not
                    // enrich it — Go to definition behaving exactly as it always has.
                    interface_method: false,
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
                            // Filled in by `cmd::diagnostics`, for the reason the twin in
                            // `DefinitionAnswer::Found` above states: parsing Go is not this
                            // layer's job.
                            interface_method: false,
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

    /// The completion list at a position. (M25)
    ///
    /// **Blocks for up to `timeout`.** Blocking pool only, and the handles lock is cloned out and
    /// dropped before the wait — the same two rules [`Self::definition`] states at length, and
    /// they bite harder here: this runs on a keystroke, so holding the lock would stall the
    /// project's diagnostics pump every time somebody types.
    ///
    /// `before` is the character immediately left of the caret, or `None` at the start of a line.
    /// The webview sends it and this function decides what it *means*, because the meaning is the
    /// server's declared `triggerCharacters` list and that list lives here. Handing the list to
    /// the webview instead would have been a whole plumbing path — an event, a per-language
    /// cache, an invalidation on restart — to move a fact three lines away from its only reader.
    ///
    /// Never returns an error type, like every one of its neighbours. Unlike them, the answer is
    /// usually never read by anyone: an implicit query that fails is dropped silently in the
    /// webview, because a popup that opens by itself cannot report its failures without becoming
    /// a notification storm. See [`cide_ipc::CompletionAnswer`].
    pub fn completion(
        &self,
        path: &std::path::Path,
        line: u32,
        column: u32,
        before: Option<char>,
        timeout: std::time::Duration,
    ) -> cide_ipc::CompletionAnswer {
        use cide_ipc::CompletionAnswer;

        let (server, requester) = match self.requester_for(path) {
            Ok(pair) => pair,
            Err(missing) => {
                return CompletionAnswer::Unavailable {
                    reason: missing.sentence("Code completion"),
                };
            }
        };

        /*
         * The capability, and the trigger list, read together in one pass over the handles.
         *
         * `Some(false)` is refused outright for `usages`' reason — asking a server that declared
         * no `completionProvider` spends the whole deadline to learn what the handshake already
         * said. `None` is *not* refused: rust-analyzer spends its first two minutes indexing and
         * a user types during them.
         *
         * The lock is taken once for both. Two separate takes would be two chances to observe a
         * restart half-way through and pair one life's capability with another's triggers.
         */
        let (known, triggers) = {
            let handles = self.handles.lock();
            match handles.iter().find(|handle| handle.server() == server) {
                Some(handle) => (handle.supports_completion(), handle.completion_triggers()),
                None => (None, std::sync::Arc::from(Vec::new())),
            }
        };
        if known == Some(false) {
            return CompletionAnswer::Unavailable {
                reason: format!(
                    "{} does not offer completion. It answered the handshake without \
                     `completionProvider`.",
                    server.binary()
                ),
            };
        }

        let mut params = position_params(path, line, column);
        /*
         * `context`, which cide declared `contextSupport: true` for.
         *
         * `2` is `TriggerCharacter` and `1` is `Invoked`. The distinction is not decoration: a
         * server may legitimately answer a `.` differently from a Ctrl+Space at the same
         * position — offering only members for the first and everything in scope for the second —
         * and a client that always said `Invoked` would get the second answer for both.
         *
         * A character the server did **not** declare is `Invoked`, not `TriggerCharacter` with
         * the character attached. Claiming a trigger the server never asked for is a statement
         * about its own configuration that we are in no position to make.
         */
        params["context"] = match before.filter(|char| triggers.contains(char)) {
            Some(char) => serde_json::json!({
                "triggerKind": 2,
                "triggerCharacter": char.to_string(),
            }),
            None => serde_json::json!({ "triggerKind": 1 }),
        };

        // Supersede whatever was outstanding — a newer keystroke's list is the only one anybody
        // will read, and the old request is a whole-crate scan nobody is waiting for. Never
        // touches `usages_in_flight`; see the field.
        self.cancel_completion();
        let slot = &self.completion_in_flight;
        let mine = std::cell::Cell::new(0i64);
        let answer = requester.request_tracked("textDocument/completion", params, timeout, |id| {
            mine.set(id);
            *slot.lock() = Some((requester.clone(), id));
        });
        // Cleared only if it is still ours — the race `usages` writes out at length, and it is
        // reached far more often here because the superseding request is one keystroke away.
        {
            let mut in_flight = slot.lock();
            if in_flight.as_ref().map(|(_, id)| *id) == Some(mine.get()) {
                *in_flight = None;
            }
        }

        match answer {
            Ok(value) => match cide_lsp::convert::completion(&value) {
                Some(reply) => {
                    let token = self.remember_completion(server, reply.resolvable);
                    CompletionAnswer::Items {
                        token,
                        items: reply.items,
                        incomplete: reply.incomplete,
                        truncated: reply.truncated,
                    }
                }
                // `null` — the server has nothing to offer here. An empty *list* is the same
                // thing to this caller, unlike `references` where the two mean opposite things,
                // because "no symbol here" and "no completions here" both draw no popup.
                None => CompletionAnswer::Items {
                    // No rows, so nothing can name this reply. A token is still issued rather
                    // than reusing zero, because "the reply with no resolvable rows" must not
                    // collide with a real one in the cache.
                    token: self.remember_completion(server, Vec::new()),
                    items: Vec::new(),
                    incomplete: false,
                    truncated: false,
                },
            },
            // Superseded by the next keystroke, which is the common outcome rather than an
            // unusual one. It must stay distinguishable from an empty list all the same: the
            // webview drops a cancellation without touching the popup, where an empty answer
            // would close it — and a popup that blinks shut on every third character is the
            // symptom this branch exists to prevent.
            Err(cide_lsp::RequestError::Cancelled) => CompletionAnswer::Unavailable {
                reason: "The completion request was superseded.".to_string(),
            },
            Err(cide_lsp::RequestError::Timeout) => CompletionAnswer::Unavailable {
                reason: format!(
                    "{} did not answer in time — it is probably still indexing. Try again in a moment.",
                    server.binary()
                ),
            },
            Err(error) => CompletionAnswer::Unavailable {
                reason: format!("{}: {error}", server.binary()),
            },
        }
    }

    /// File this reply's originals and hand back the token that names them.
    fn remember_completion(&self, server: Server, items: Vec<serde_json::Value>) -> u32 {
        let token = self
            .completion_token
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut cache = self.completion_cache.lock();
        cache.push(CompletionReplyCache {
            token,
            server,
            items,
        });
        // Oldest first out. A `Vec` and `remove(0)` rather than a `VecDeque`, because the depth
        // is two: the shift is one move and the type stays the one a reader can hold in their
        // head while checking the staleness rule below.
        while cache.len() > COMPLETION_CACHE_DEPTH {
            cache.remove(0);
        }
        token
    }

    /// Fetch the deferred half of one completion item — the `use` line, the `import` block. (M25)
    ///
    /// **Blocks for up to `timeout`.** Blocking pool only, same as its neighbours. Unlike them it
    /// runs on an *accept* rather than on a keystroke, so it happens once per completion the user
    /// commits to and its latency is directly in front of a Tab press.
    ///
    /// `token` and `index` name a row of a reply this project handed out. A token that is no
    /// longer held is [`cide_ipc::CompletionResolveAnswer::Unavailable`] and **not** an empty edit
    /// list — see that type for why the difference is the whole point.
    pub fn resolve_completion(
        &self,
        token: u32,
        index: u32,
        timeout: std::time::Duration,
    ) -> cide_ipc::CompletionResolveAnswer {
        use cide_ipc::CompletionResolveAnswer as Answer;

        // The item is cloned out and the guard dropped before anything waits — the same rule
        // `requester_for` follows about `handles`, and for the same reason: the pump takes this
        // lock too, through `remember_completion`, every time a completion answers.
        let found = {
            let cache = self.completion_cache.lock();
            cache
                .iter()
                .find(|reply| reply.token == token)
                .and_then(|reply| {
                    reply
                        .items
                        .get(index as usize)
                        .map(|item| (reply.server, item.clone()))
                })
        };
        let Some((server, item)) = found else {
            return Answer::Unavailable {
                reason: "That completion is no longer available — the list moved on before it \
                         could be accepted."
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
            return Answer::Unavailable {
                reason: format!("{} is no longer running.", server.binary()),
            };
        };

        // Deliberately *not* tracked in `completion_in_flight`. That slot exists so a newer
        // keystroke supersedes an older list; a resolve is the opposite kind of request — the
        // user has already chosen, nothing supersedes it, and cancelling it would abandon an
        // edit the accept is waiting on.
        match requester.request("completionItem/resolve", item, timeout) {
            Ok(value) => match cide_lsp::convert::resolved_edits(&value) {
                Some(extra_edits) => Answer::Edits { extra_edits },
                None => Answer::Unavailable {
                    reason: format!("{} answered the resolve with no item.", server.binary()),
                },
            },
            Err(cide_lsp::RequestError::Timeout) => Answer::Unavailable {
                reason: format!(
                    "{} did not finish working out the edit in time.",
                    server.binary()
                ),
            },
            Err(error) => Answer::Unavailable {
                reason: format!("{}: {error}", server.binary()),
            },
        }
    }

    /// Withdraw the outstanding completion, if there is one.
    ///
    /// Called by the superseding request above and by the webview when the popup closes. Releases
    /// the blocking-pool thread *and* sends `$/cancelRequest`, for [`Self::cancel_usages`]'
    /// reason — with the volume argument on top: a user typing at speed supersedes a request per
    /// keystroke, and without the protocol half each one leaves rust-analyzer computing a list
    /// nobody will read, several deep, each starving the one the user is actually waiting for.
    pub fn cancel_completion(&self) {
        let outstanding = self.completion_in_flight.lock().take();
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
    /// Documentation for a subject, by whichever road the owning server has. (M60)
    ///
    /// **Blocks for up to `timeout`.** Call it from the blocking pool, never from a Tauri command
    /// worker — see `cmd::diagnostics::docs_lookup`.
    ///
    /// # What is remembered, and what is asked again
    ///
    /// A page whose provider can name it — a Godot native symbol, `#ref/class/Node` — is kept by
    /// that name, so the tab that opens on the answer draws without a second round trip and a
    /// reference followed twice is fetched once. A page asked for *by position* is never kept:
    /// the buffer under that position moves, and a remembered page would describe the symbol
    /// that used to be there. The answer's subject is the remembered one, which is how a
    /// position lookup that landed on a native symbol comes back with a name the tab can carry.
    ///
    /// Never returns an error type: every failure is a sentence, `definition`'s rule.
    pub fn documentation(
        &self,
        subject: &cide_ipc::DocsSubject,
        timeout: std::time::Duration,
    ) -> cide_ipc::DocsAnswer {
        use cide_ipc::{DocsAnswer, DocsSubject};
        use cide_lsp::docs::{self, Outcome};

        if let DocsSubject::Reference { .. } = subject {
            let cached = self
                .docs_cache
                .lock()
                .iter()
                .find(|(known, _)| known == subject)
                .map(|(_, page)| page.clone());
            if let Some(page) = cached {
                return DocsAnswer::Found {
                    page: Box::new(page),
                    subject: subject.clone(),
                };
            }
        }
        let (server, requester) = match subject {
            DocsSubject::Position { path, .. } => match self.requester_for(path) {
                Ok(pair) => pair,
                Err(missing) => {
                    return DocsAnswer::Unavailable {
                        reason: missing.sentence("Quick documentation"),
                    };
                }
            },
            DocsSubject::Reference { source, .. } => match self.requester_by_source(source) {
                Some(pair) => pair,
                None => {
                    return DocsAnswer::Unavailable {
                        reason: format!(
                            "{source} is not running for this project. Following a reference \
                             needs it."
                        ),
                    };
                }
            },
        };
        let outcome = match subject {
            DocsSubject::Position {
                path,
                line,
                column,
                word,
            } => docs::lookup(&requester, path, *line, *column, word, timeout),
            DocsSubject::Reference {
                ref_kind, target, ..
            } => docs::follow(&requester, ref_kind, target, timeout),
        };
        match outcome {
            Outcome::Page(page) => {
                let subject = page
                    .reference
                    .as_deref()
                    .and_then(docs::parse_reference)
                    .map(|(kind, target)| DocsSubject::Reference {
                        source: page.source.clone(),
                        ref_kind: kind.to_string(),
                        target: target.to_string(),
                    })
                    .unwrap_or_else(|| subject.clone());
                if let DocsSubject::Reference { .. } = subject {
                    let mut cache = self.docs_cache.lock();
                    cache.retain(|(known, _)| *known != subject);
                    if cache.len() >= DOCS_CACHE_DEPTH {
                        cache.remove(0);
                    }
                    cache.push((subject.clone(), page.clone()));
                }
                DocsAnswer::Found {
                    page: Box::new(page),
                    subject,
                }
            }
            Outcome::Nothing => DocsAnswer::NotFound,
            // `definition`'s sentence: while a server indexes, this is the expected answer for
            // the first minute of a session, and "nothing here" would be a lie.
            Outcome::Failed(cide_lsp::RequestError::Timeout) => DocsAnswer::Unavailable {
                reason: format!(
                    "{} did not answer in time — it is probably still indexing. Try again in a \
                     moment.",
                    server.binary()
                ),
            },
            Outcome::Failed(error) => DocsAnswer::Unavailable {
                reason: format!("{}: {error}", server.binary()),
            },
        }
    }

    /// The running server registered under `source` — a page's own `source`, for following a
    /// reference back to the server that wrote it.
    fn requester_by_source(&self, source: &str) -> Option<(Server, cide_lsp::Requester)> {
        let handles = self.handles.lock();
        handles
            .iter()
            .find(|handle| handle.server().binary() == source)
            .map(|handle| (handle.server(), handle.requester()))
    }

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

    /// Files changed on disk, by somebody who is not this editor. (M18)
    ///
    /// The other half of document sync, and the half that was missing. `cmd::diagnostics`'
    /// `did_open`/`did_change`/`did_save` cover the buffer the *user* is typing in; this covers
    /// everything else — an agent's `Edit`, a `git checkout`, a `cargo fmt`, a `go mod tidy`.
    ///
    /// Runs on the **watcher thread** (see `files::on_watch_event`), so it does three cheap things
    /// and parks the expensive one:
    ///
    /// 1. **Marks the store dirty.** The panel then says "may be out of date" on rows whose file
    ///    has moved underneath them, which is the honest state until the analyser re-reports. A
    ///    row that silently keeps a wrong line number is the reported bug.
    /// 2. **Tells gopls at once**, with `workspace/didChangeWatchedFiles`. gopls has no watcher of
    ///    its own and this notification *is* how it learns; there is nothing to debounce, because
    ///    one watcher event already carries a whole batch of paths and gopls does its own
    ///    throttling.
    /// 3. **Parks a flycheck kick** for rust-analyzer on the [`Kick`] coalescer. That one is a
    ///    `cargo check` of the workspace and must not be started per event.
    ///
    /// Paths are split per server rather than broadcast: `go.mod` matters to gopls and `Cargo.toml`
    /// to rust-analyzer, and neither has any use for the other's.
    pub fn files_changed(&self, paths: &[PathBuf]) {
        let mut go: Vec<&PathBuf> = Vec::new();
        let mut rust = false;
        for path in paths {
            // `if let` rather than a `match` on the whole `Option<Server>`: since M22 a `Server` is
            // an index into a table an extension can extend, so a `match` here would need a `_`
            // arm and would stop being a claim about coverage. The two named servers are the two
            // this function has a batching rule for; a contributed server's changed files reach it
            // through `notify_server` below like everything else.
            if let Some(server) = owner_of(path) {
                if server == Server::GOPLS {
                    go.push(path);
                } else if server == Server::RUST_ANALYZER {
                    rust = true;
                }
            }
        }
        if go.is_empty() && !rust {
            return;
        }

        // Only paths something has already reported on can be marked — see `mark_dirty` — so this
        // is bounded by the length of the problems list rather than by the size of the burst.
        let marked = {
            let mut store = self.store.lock();
            store.mark_dirty(paths.iter().filter_map(|p| p.to_str()))
        };
        self.kick.schedule(marked);

        if !go.is_empty() {
            /*
             * `2` changed, `3` deleted — LSP's `FileChangeType`.
             *
             * The watcher does not say which, so the disk is asked. One `stat` per changed Go file
             * is cheap next to what the alternative costs: telling gopls a deleted file merely
             * "changed" leaves its diagnostics standing for a file that is gone, which is the
             * same class of stale row this whole change is about. `1` (created) is deliberately
             * not distinguished from `2`: nothing downstream treats them differently, and telling
             * them apart would need a memory of what existed before the event.
             */
            let changes: Vec<(String, u8)> = go
                .iter()
                .map(|path| {
                    let kind = if path.exists() { 2 } else { 3 };
                    (cide_lsp::convert::path_to_uri(path), kind)
                })
                .collect();
            self.notify_server(Server::GOPLS, |session| {
                session.did_change_watched_files(changes.clone())
            });
        }
    }

    /// Re-run everything, now, because the user asked. (M18)
    ///
    /// The Problems panel's *Re-run analysis* button and the `problems.refresh` command. Returns
    /// the sentence to show, because a control whose whole effect happens in another process over
    /// the next few seconds is otherwise indistinguishable from a control that is wired to
    /// nothing — which is this repository's named recurring defect and the user's actual
    /// complaint.
    ///
    /// # What it costs, and why it is not a restart
    ///
    /// A restart of rust-analyzer on a large workspace is minutes of re-indexing, and it is
    /// already available per source in the panel's footer (`diagnostics.restart`, unchanged). This
    /// is the cheap one: `rust-analyzer/runFlycheck` re-runs `cargo check` without touching the
    /// index, and a `didChangeWatchedFiles` sweep makes gopls re-read the files it has findings
    /// for. Seconds rather than minutes, and it is what the user means by "re-run it".
    ///
    /// **The staleness marks are not cleared here, and that is the point of them.** An earlier
    /// version cleared them wholesale right after sending the kick, reasoning that the button
    /// would otherwise look inert. The kick is asynchronous: `runFlycheck` is a `cargo check`
    /// and the gopls sweep is a re-read, so results are seconds to minutes away. Clearing on
    /// send therefore took the "may be out of date" mark off every row while every row was
    /// still exactly as out of date as it had been — which is the state the mark exists to
    /// describe, and the state the user reported landing in when a click on a fixed diagnostic
    /// navigated them into unrelated comments.
    ///
    /// `DiagnosticStore::publish` already clears a path's mark when its source speaks about it
    /// again, so the marks come down one file at a time as the answers actually arrive, and a
    /// row that nothing re-reported keeps saying so. What stops the button looking inert is the
    /// sentence it returns, not the silent removal of a warning that was still true.
    pub fn refresh(&self, app: &tauri::AppHandle) -> String {
        let running: Vec<Server> = self
            .handles
            .lock()
            .iter()
            .map(cide_lsp::LspHandle::server)
            .collect();
        if running.is_empty() {
            return "No language server is running for this project, so there is nothing to \
                    re-run. Rust needs rust-analyzer and Go needs gopls on PATH."
                .to_string();
        }

        if running.contains(&Server::RUST_ANALYZER) {
            self.notify_server(Server::RUST_ANALYZER, cide_lsp::Session::run_flycheck);
        }
        if running.contains(&Server::GOPLS) {
            /*
             * gopls has no "re-check everything" primitive a client may call, so the honest thing
             * is to tell it what a client normally tells it: these files may have changed. It
             * re-reads them from disk and re-diagnoses, which is the effect being asked for.
             *
             * Every path gopls has *looked at* — an empty publish counts, which is what
             * `known_paths` gives — rather than only the ones with findings. A file whose last
             * error was fixed elsewhere is exactly the file a user presses this button about.
             */
            let changes: Vec<(String, u8)> = self
                .store
                .lock()
                .known_paths()
                .into_iter()
                .map(PathBuf::from)
                .filter(|path| owner_of(path) == Some(Server::GOPLS) && path.exists())
                .map(|path| (cide_lsp::convert::path_to_uri(&path), 2u8))
                .collect();
            if !changes.is_empty() {
                self.notify_server(Server::GOPLS, |session| {
                    session.did_change_watched_files(changes.clone())
                });
            }
        }

        self.kick.clear();
        crate::emit::diagnostics(app, self.project);

        let names: Vec<String> = running.iter().map(|s| s.binary()).collect();
        format!(
            "Re-running {}. Results replace the current list as they arrive.",
            match names.as_slice() {
                [one] => one.clone(),
                _ => names.join(" and "),
            }
        )
    }

    /// Send one notification to every handle for `server`.
    ///
    /// The shape [`Self::notify_document`] already uses — a throwaway [`cide_lsp::Session`] to
    /// build the message, because the document and workspace builders are pure and the live
    /// session never leaves the supervisor thread — with the per-path server lookup dropped,
    /// since these messages are *about* the server rather than about one file.
    fn notify_server(
        &self,
        server: Server,
        message: impl Fn(&cide_lsp::Session) -> cide_lsp::Effect,
    ) {
        for handle in self.handles.lock().iter() {
            if handle.server() != server {
                continue;
            }
            let (session, _) = cide_lsp::Session::new(&self.roots, server);
            if let cide_lsp::Effect::Send(value) = message(&session) {
                handle.send(value);
            }
        }
    }

    /// Where the thing under the caret is *implemented*, as opposed to declared. (M18)
    ///
    /// # Why this is a second request and not a smarter first one
    ///
    /// The user's report was that Go to definition in Go lands on the interface. It does, and
    /// gopls is right: `textDocument/definition` on a call through an interface resolves to the
    /// interface's method, because that is where the thing being called is declared. "Take me to
    /// the concrete one" is `textDocument/implementation`, a different question, and cide had no
    /// gesture that asked it.
    ///
    /// Making Ctrl+B ask *this* first and fall back was the obvious alternative and it regresses
    /// Rust: rust-analyzer answers `implementation` on a struct name with its `impl` blocks and on
    /// a trait with its implementors, so Ctrl+click on an ordinary type name would stop opening
    /// the declaration. A separate bindable id is what this codebase already reached for when
    /// Ctrl+click's discriminator could guess wrong — see `navigate.usages` in
    /// `cide_core::commands` — and it is the same answer here.
    ///
    /// Returns [`cide_ipc::UsagesAnswer`] and reuses the Find usages popup wholesale, because the
    /// shape of the answer is identical: an interface with many implementors is the common case,
    /// and "several results, pick one" is a picker that already exists and is already tested.
    ///
    /// **Blocks for up to `timeout`.** Blocking pool only, handles lock dropped before the wait —
    /// the rules [`Self::usages`] states, which apply here unchanged. It shares
    /// [`Self::usages_in_flight`] with that method deliberately: one popup, one outstanding
    /// request, so Escape cancels whichever is running without the webview naming it.
    pub fn implementations(
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
                    reason: missing.sentence("Go to implementation"),
                };
            }
        };

        // The one case worth refusing outright, and `None` is deliberately not it — the whole
        // argument is written out in `usages`, and it is the same three-state reading.
        let known = {
            let handles = self.handles.lock();
            handles
                .iter()
                .find(|handle| handle.server() == server)
                .and_then(cide_lsp::LspHandle::supports_implementation)
        };
        if known == Some(false) {
            return UsagesAnswer::Unavailable {
                reason: format!(
                    "{} does not offer Go to implementation. It answered the handshake without \
                     `implementationProvider`.",
                    server.binary()
                ),
            };
        }

        // No `context` member: `textDocument/implementation` takes a bare `TextDocumentPositionParams`,
        // unlike `references`, so there is no `includeDeclaration` to set. The origin is still
        // filtered out of the reply by `rows_from` — an "implementation" at the caret is where the
        // user already is.
        let params = position_params(path, line, column);

        self.cancel_usages();
        let slot = &self.usages_in_flight;
        let mine = std::cell::Cell::new(0i64);
        let answer =
            requester.request_tracked("textDocument/implementation", params, timeout, |id| {
                mine.set(id);
                *slot.lock() = Some((requester.clone(), id));
            });
        // Cleared only if it is still ours — the same race `usages` documents: a second gesture
        // that superseded us has already put its own id in the slot, and a blind `take` would
        // leave Escape with nothing to cancel.
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
                // `null`: there is no symbol at this position at all. The caller falls back to Go
                // to definition on this, which is what makes the command always do *something*.
                None => UsagesAnswer::NotFound,
            },
            /*
             * A rejected *question*, folded into "there is nothing here". (M18)
             *
             * Not a guess: `gopls_goes_to_the_implementation_rather_than_the_interface` caught it
             * on the first run. gopls answers `textDocument/implementation` on a position that is
             * not a type with a JSON-RPC **error** — literally `s is a var, not a type` — where
             * the spec's own answer for "nothing to report" is `null`. Rendered as
             * `Unavailable`, a Ctrl+Alt+B on an ordinary local variable would put that sentence
             * on screen as though the server were broken.
             *
             * `NotFound` is what the caller falls back to Go to definition on, which is the
             * useful thing to do with a caret the question does not apply to — and it is what
             * makes this command always do *something* rather than being silent on the majority
             * of positions it is pressed at.
             *
             * The server's own words are kept, in the log rather than on screen: a genuinely
             * broken server still reaches here, and throwing its explanation away would leave
             * nothing to diagnose it with. `Timeout`, `Cancelled` and `ServerGone` are untouched
             * — those are failures of the *transport*, not answers about the position, and each
             * keeps its own sentence.
             */
            Err(cide_lsp::RequestError::Failed(message)) => {
                tracing::debug!(
                    target: "cide::lsp",
                    server = server.binary(),
                    %message,
                    "the server refused an implementation query at this position"
                );
                UsagesAnswer::NotFound
            }
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

    /// Reformat `text` with this file's language server. (M26)
    ///
    /// **Blocks for up to `timeout`.** Blocking pool only, and the `handles` lock is taken to
    /// clone a `Requester` and then dropped — the rule [`Self::definition`] states.
    ///
    /// `text` is the buffer as the caller has it, and the caller has already flushed its
    /// `didChange`, so the server is formatting the same bytes. Passing it in is what lets the
    /// reply's edits be applied *here*, where the (line, UTF-16 column) → offset conversion
    /// already lives, rather than a second time in TypeScript. See
    /// `cide_lsp::convert::apply_text_edits`.
    ///
    /// `range` narrows the request to a selection **only if the server said it could** — the
    /// probe is `Some(true)` and nothing weaker. `None` there means the handshake has not
    /// finished, and the honest move is the whole document: range formatting is an optimisation
    /// over a fallback that always works, so gambling a round trip on a method the server may
    /// answer with `MethodNotFound` buys nothing. This is the one capability read that way
    /// round, and `LspHandle::supports_range_formatting` says so at its definition.
    pub fn format(
        &self,
        path: &std::path::Path,
        text: &str,
        range: Option<cide_ipc::FormatRange>,
        options: serde_json::Value,
        timeout: std::time::Duration,
    ) -> cide_ipc::FormatAnswer {
        use cide_ipc::FormatAnswer;

        let (server, requester) = match self.requester_for(path) {
            Ok(pair) => pair,
            Err(missing) => {
                return FormatAnswer::Unavailable {
                    reason: missing.sentence("Reformat code"),
                };
            }
        };

        // `Some(false)` is a real refusal and worth making immediately: an extension may
        // contribute a server that does not format at all, and a twenty-second wait ending in
        // "probably still indexing" would be a lie about it. `None` is *not* a refusal — see
        // `LspHandle::supports_formatting`.
        let (formats, ranges) = {
            let handles = self.handles.lock();
            let handle = handles.iter().find(|handle| handle.server() == server);
            (
                handle.and_then(cide_lsp::LspHandle::supports_formatting),
                handle.and_then(cide_lsp::LspHandle::supports_range_formatting),
            )
        };
        if formats == Some(false) {
            return FormatAnswer::Unavailable {
                reason: format!(
                    "{} does not offer formatting. It answered the handshake without \
                     `documentFormattingProvider`.",
                    server.binary()
                ),
            };
        }

        let uri = cide_lsp::convert::path_to_uri(path);
        let (method, params) = match range.filter(|_| ranges == Some(true)) {
            Some(range) => (
                "textDocument/rangeFormatting",
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "range": range_params(range),
                    "options": options,
                }),
            ),
            None => (
                "textDocument/formatting",
                serde_json::json!({
                    "textDocument": { "uri": uri },
                    "options": options,
                }),
            ),
        };

        match requester.request(method, params, timeout) {
            Ok(value) => match cide_lsp::convert::apply_text_edits(text, &value) {
                Some(formatted) if formatted == text => FormatAnswer::Unchanged {
                    by: server.binary(),
                },
                Some(text) => FormatAnswer::Formatted {
                    text,
                    by: server.binary(),
                },
                // A reply this does not understand. Reported rather than folded into
                // "no change", which would make a protocol disagreement look exactly like an
                // already-formatted file and hide the feature being broken for ever.
                None => FormatAnswer::Unavailable {
                    reason: format!(
                        "{} answered with something other than a list of edits.",
                        server.binary()
                    ),
                },
            },
            // Its own sentence, for the reason `definition` gives: while the server is indexing
            // this is the *expected* answer for the first minute of a session, and a generic
            // failure would teach the user the formatter does not work.
            Err(cide_lsp::RequestError::Timeout) => FormatAnswer::Unavailable {
                reason: format!(
                    "{} did not answer in time — it is probably still indexing. Try again in a moment.",
                    server.binary()
                ),
            },
            Err(error) => FormatAnswer::Unavailable {
                reason: format!("{}: {error}", server.binary()),
            },
        }
    }

    /// Restart one source after it gave up.
    /// Start any newly contributed server, and drop any that is no longer contributed. (M22)
    ///
    /// # Why this exists
    ///
    /// Without it, installing an extension that contributes a language server did nothing until
    /// the project was reopened — the servers were chosen once, when `ProjectDiagnostics` was
    /// created. That is not a limitation a user can see and work around; it is the feature
    /// appearing to be broken. `sqls` reported nothing, had no row in the Problems footer, and
    /// therefore had no Restart button either, which is three symptoms of one cause.
    ///
    /// # What it deliberately does not do
    ///
    /// It does not restart anything that is already running. A server keeps its handle if it is
    /// still contributed, so enabling a YAML extension does not cost a rust-analyzer re-index —
    /// which on a large workspace is minutes and several gigabytes, and was the objection to the
    /// simpler "stop everything and start again".
    ///
    /// Safe to call with sessions live because `cide_lsp::discover`'s table is append-only: a
    /// `Server` a running session holds still resolves to the row it was minted for.
    pub fn sync(&self, app: &tauri::AppHandle) {
        let before = self.handles.lock().len();
        sync_servers(
            &self.store,
            &self.handles,
            &self.roots,
            &binary_choices(app),
            server_tuning(app),
        );
        if self.handles.lock().len() != before {
            crate::emit::diagnostics(app, self.project);
        }
    }

    /// Fold findings from something that is not a language server into this project's store. (M22)
    ///
    /// The road an extension's diagnostics take. Deliberately the *same* store the servers publish
    /// into, rather than a second list merged in the panel: `DiagnosticStore` is where the source
    /// filter, the staleness marks and the cap are applied, and a second path around it would be a
    /// second set of answers to "how many errors are there" — which is the number on the rail.
    ///
    /// Replaces everything that source has said about that path, exactly as `publishDiagnostics`
    /// does for a server. An extension that wants to clear a file publishes an empty list, which
    /// is a real message and not the absence of one.
    pub fn publish_external(
        &self,
        app: &tauri::AppHandle,
        source: cide_ipc::DiagnosticSourceId,
        abs_path: String,
        items: Vec<cide_ipc::Diagnostic>,
    ) {
        // The relative path is derived here and not trusted from the payload. A worker is handed
        // an absolute path and knows nothing about roots, so the `path` field it sent is a copy of
        // `abs_path`; the panel groups on this one, and a multi-root project prefixes it with a
        // root label. Deriving it is the same call the server path makes, so a contributed row and
        // a rust-analyzer row for the same file land under one heading.
        let mut items = items;
        for item in &mut items {
            item.path = relative(&self.roots, &item.abs_path);
        }
        {
            let mut store = self.store.lock();
            // A source that has published is a source that is running, which is what makes its row
            // appear in the panel's footer with a Restart beside it. Without this it would report
            // findings while its own row said nothing had looked.
            store.set_status(source.clone(), SourceStatus::Ready);
            store.publish(source, abs_path, items);
        }
        crate::emit::diagnostics(app, self.project);
    }

    pub fn restart(&self, app: &tauri::AppHandle, source: cide_ipc::DiagnosticSourceId) {
        // tree-sitter and Claude are not processes and have nothing to restart. Since M22 the
        // reverse mapping is a registry lookup rather than a `match`, so a server that arrived
        // from a manifest is restartable on exactly the same road as the two builtins — which is
        // what the panel's Restart button needed, and what a `match` here would have silently
        // refused it.
        let Some(server) = self
            .handles
            .lock()
            .iter()
            .map(LspHandle::server)
            .find(|s| s.source() == source)
            .or_else(|| {
                cide_lsp::discover::servers()
                    .into_iter()
                    .find(|s| s.source() == source)
            })
        else {
            return;
        };
        respawn(
            app,
            self.project,
            &self.store,
            &self.handles,
            &self.roots,
            server,
        );
    }

    /// Drop every handle and wait each shutdown ladder out — returning is the proof the
    /// processes are gone, which is what [`DiagnosticsRegistry::invalidate_caches`] needs
    /// before it deletes anything the dying servers might still write.
    ///
    /// The handles are taken in one short lock and dropped outside it: a ladder can take
    /// seconds, and the pump's watchdog takes this same lock on its tick.
    pub fn stop_servers(&self) {
        let dropped: Vec<LspHandle> = std::mem::take(&mut *self.handles.lock());
        for handle in dropped {
            let source = handle.server().source();
            drop(handle);
            self.store.lock().clear_source(source);
        }
    }
}

/// Drop one server's handle and start a fresh one, on the spawn thread.
///
/// The shared tail of [`ProjectDiagnostics::restart`] (the panel's Restart button, the
/// settings row's Builtin/System flip) and the memory watchdog in [`pump`] — a free function
/// because the pump holds the pieces, not the `ProjectDiagnostics`. Order is load-bearing:
/// the old handle drops **first** (its `Drop` runs the shutdown ladder), or two servers
/// briefly index the same workspace, which on rust-analyzer is 2–8 GB.
///
/// The binary choice is read fresh, not captured at project open: a respawn that used
/// yesterday's choice would make the settings row a lie.
fn respawn(
    app: &tauri::AppHandle,
    project: ProjectId,
    store: &Arc<Mutex<DiagnosticStore>>,
    handles: &Arc<Mutex<Vec<LspHandle>>>,
    roots: &[PathBuf],
    server: Server,
) {
    let source = server.source();
    handles.lock().retain(|h| h.server() != server);
    store.lock().clear_source(source.clone());
    let roots = roots.to_vec();
    let choice = binary_choices(app)
        .get(&server.binary())
        .copied()
        .unwrap_or_default();
    let tuning = server_tuning(app);
    let started = cide_core::child_env::on_spawn_thread(move || {
        LspHandle::start_with(server, roots, choice, tuning)
    });
    match started {
        Ok(handle) => {
            // The same gap `sync_servers` covers, and it was missed here: `clear_source`
            // above emptied the row, and without this the emit below broadcasts a store
            // with no source at all — which the whole chain renders as "no language server
            // is running" until the new session's own `Scanning` arrives. A watchdog or
            // settings-change respawn made that a recurring flash, not a one-shot.
            store.lock().set_status(
                source,
                SourceStatus::Scanning {
                    detail: format!("restarting {}", server.binary()),
                    percentage: None,
                },
            );
            handles.lock().push(handle);
        }
        Err(error) => store.lock().set_status(
            source,
            SourceStatus::Unavailable {
                reason: error.to_string(),
            },
        ),
    }
    crate::emit::diagnostics(app, project);
}

/// A process's resident memory, from `/proc/<pid>/statm`. `None` off Linux, and for any pid
/// that is gone — which the watchdog treats as an ordinary answer, because it races the
/// supervisor zeroing the pid slot by design.
fn resident_memory_bytes(pid: u32) -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let statm = std::fs::read_to_string(format!("/proc/{pid}/statm")).ok()?;
        let resident_pages: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
        // The page size is a runtime fact, but not one that changes: 4096 everywhere cide
        // has ever run. `sysconf` would cost a libc call per poll to defend against a
        // hypothetical huge-page-default kernel, where this number being wrong makes the
        // limit proportionally wrong, not the mechanism.
        Some(resident_pages * 4096)
    }
    #[cfg(not(target_os = "linux"))]
    {
        // No `/proc` — the watchdog simply never fires. `docs/platforms.md` records
        // this as one of the macOS gaps: the honest alternative is `proc_pidinfo`, which
        // needs a Mac in front of the person writing it.
        let _ = pid;
        None
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

/// A [`cide_ipc::FormatRange`] in LSP's units. (M26)
///
/// 0-based on the wire, and the column stays UTF-16 — the same conversion [`position_params`]
/// makes, `saturating_sub` included, because a selection starting at column 1 must not underflow
/// to `u32::MAX` and ask the server to format from four billion characters in.
fn range_params(range: cide_ipc::FormatRange) -> serde_json::Value {
    serde_json::json!({
        "start": {
            "line": range.start_line.saturating_sub(1),
            "character": range.start_column.saturating_sub(1),
        },
        "end": {
            "line": range.end_line.saturating_sub(1),
            "character": range.end_column.saturating_sub(1),
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
/// Bring a project's running servers into line with the registry.
///
/// Additive and subtractive, never a restart: a server already running is left exactly alone, one
/// that is newly contributed is started, and one that has stopped being contributed has its handle
/// dropped — whose `Drop` runs the shutdown ladder — and its findings cleared, because a source
/// that is no longer running must not leave a stale list behind under a row that says nothing is
/// wrong.
/// The user's say over which build each server runs, read from stored settings.
///
/// Read fresh at every call rather than held anywhere: the value lives in the workspace's
/// settings, a spawn is rare, and a cached copy is a copy that survives the settings write it
/// exists to honour. Before the workspace state exists (tests, early startup) the answer is
/// the default for everything — which is also what an empty map means.
fn binary_choices(
    app: &tauri::AppHandle,
) -> std::collections::BTreeMap<String, cide_ipc::settings::ServerBinaryChoice> {
    app.try_state::<crate::workspace_state::WorkspaceState>()
        .map(|state| state.with(|ws| ws.settings.inspections.server_binaries.clone()))
        .unwrap_or_default()
}

/// The handshake tuning, read fresh like [`binary_choices`] and for the same reasons. The
/// default (all zeros) is "the fork's own defaults", which is also what early startup and
/// tests get before workspace state exists.
fn server_tuning(app: &tauri::AppHandle) -> cide_lsp::config::Tuning {
    app.try_state::<crate::workspace_state::WorkspaceState>()
        .map(|state| {
            state.with(|ws| cide_lsp::config::Tuning {
                index_working_set_pct: ws.settings.inspections.server_index_working_set_pct,
                memory_limit_mb: ws.settings.inspections.server_memory_limit_mb,
            })
        })
        .unwrap_or_default()
}

fn sync_servers(
    store: &Arc<Mutex<DiagnosticStore>>,
    handles: &Arc<Mutex<Vec<LspHandle>>>,
    roots: &[PathBuf],
    choices: &std::collections::BTreeMap<String, cide_ipc::settings::ServerBinaryChoice>,
    tuning: cide_lsp::config::Tuning,
) {
    let wanted = cide_lsp::discover::servers();

    // Gone first, so a server whose extension was just disabled has released its child before a
    // replacement for the same binary is spawned.
    let dropped: Vec<Server> = {
        let mut held = handles.lock();
        let (keep, gone): (Vec<LspHandle>, Vec<LspHandle>) = std::mem::take(&mut *held)
            .into_iter()
            .partition(|handle| wanted.contains(&handle.server()));
        *held = keep;
        gone.iter().map(LspHandle::server).collect()
    };
    for server in dropped {
        tracing::info!(binary = %server.binary(), "sync: dropped a server no longer wanted");
        store.lock().clear_source(server.source());
    }

    let running: Vec<Server> = handles.lock().iter().map(LspHandle::server).collect();
    for server in wanted {
        if running.contains(&server) {
            continue;
        }
        // Not applicable is not "unavailable": a server whose project marker exists nowhere
        // under these roots gets **no row in the panel at all**, where one whose project is
        // here but whose binary is missing keeps its actionable sentence. A Rust workspace
        // listing gopls as a dim row was reporting cide's server table, not the workspace —
        // and `clear_source` rather than skip, because the marker can *go away* (a `go.mod`
        // deleted mid-session) and the row from that earlier life must go with it.
        if !cide_lsp::discover::applicable(server, roots) {
            store.lock().clear_source(server.source());
            continue;
        }
        // On the spawn thread, not here — see the module docs. `on_spawn_thread` blocks, and a
        // discovery probe plus a `fork` is single-digit milliseconds.
        let roots_for_server = roots.to_vec();
        let choice = choices.get(&server.binary()).copied().unwrap_or_default();
        let started = cide_core::child_env::on_spawn_thread(move || {
            LspHandle::start_with(server, roots_for_server, choice, tuning)
        });
        match started {
            Ok(handle) => {
                tracing::info!(
                    binary = %server.binary(),
                    root = ?roots.first(),
                    "sync: started a server"
                );
                // Deliberately *not* `Ready`: nothing has answered yet. `Session::new` emits its
                // own `Scanning` the moment it writes `initialize`, and this is only what the
                // store says in the gap before that arrives.
                store.lock().set_status(
                    server.source(),
                    SourceStatus::Scanning {
                        detail: format!("starting {}", server.binary()),
                        percentage: None,
                    },
                );
                handles.lock().push(handle);
            }
            Err(error) => {
                // Not an error the user has to act on unless they wanted that language: "no Cargo
                // project under this project's roots" is the ordinary answer for a Go repository.
                // The sentence is carried either way, and the panel decides how loudly to say it.
                store.lock().set_status(
                    server.source(),
                    SourceStatus::Unavailable {
                        reason: error.to_string(),
                    },
                );
            }
        }
    }
}

/// # Why this no longer goes through `cide_lang::Lang`
///
/// It used to, and that one line was the coupling M22 existed to break: `Lang` is a closed enum
/// backed by statically linked C parser tables, so *"a language cide can outline"* and *"a
/// language cide can run a server for"* were forced to be the same set. A YAML server would have
/// needed a tree-sitter grammar for YAML — an absurd price for a `--stdio` flag.
///
/// Now the path resolves to a `languageId` through the language registry, which builtins and
/// extensions both feed, and the registry says which server claims it. Outlining is unchanged and
/// still `Lang`'s job; the two questions are simply asked of two tables again.
fn server_for(path: &std::path::Path) -> Option<Server> {
    let language = language_of(path)?;
    cide_lsp::discover::by_language(&language)
}

/// Which language a path is, by the resolved registry.
///
/// The same rules `ui/src/editor/languages.ts::lookup` follows and for the same reason — a status
/// bar that says `YAML` and a server that is never sent the file would be two answers to one
/// question. Whole file name first, then the extension, and a leading dot makes a hidden file
/// rather than an extension so `.gitignore` is not looked up as a `gitignore` language.
fn language_of(path: &std::path::Path) -> Option<String> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    let languages = crate::ext_state::languages();
    for def in &languages {
        if def
            .filenames
            .iter()
            .any(|n: &String| n.eq_ignore_ascii_case(&name))
        {
            return Some(def.id.clone());
        }
    }
    let dot = name.rfind('.')?;
    if dot == 0 {
        return None;
    }
    let ext = &name[dot + 1..];
    languages
        .iter()
        .find(|def| {
            def.extensions
                .iter()
                .any(|e| e.ext.eq_ignore_ascii_case(ext))
        })
        .map(|def| def.id.clone())
}

/// Which server *cares* about a path — [`server_for`] widened to the build manifests. (M18)
///
/// A separate function rather than a widening of `server_for`, and the split is the point.
/// `server_for` answers "who would I send a `textDocument/didOpen` for this buffer to", and a
/// `Cargo.toml` has no `languageId` and must not be opened as a document. This one answers "whose
/// view of the world does a write here invalidate", which is a strictly larger set: editing
/// `go.mod` changes what gopls resolves, and editing `Cargo.toml` changes rust-analyzer's crate
/// graph, and in both cases a diagnostic in some `.rs` or `.go` file may now be wrong.
///
/// `go.work` and `Cargo.lock` are included for the same reason; `.gitignore` and the rest are not,
/// because a change there moves nothing either server has said.
fn owner_of(path: &std::path::Path) -> Option<Server> {
    if let Some(server) = server_for(path) {
        return Some(server);
    }
    // The build manifests, which have no `languageId` and so cannot come from the language
    // registry. A contributed server names its own in `projectMarkers`, which is what this falls
    // through to — so `package.json` invalidating a contributed server's view is a manifest field
    // rather than another arm here.
    let name = path.file_name()?.to_str()?;
    match name {
        "Cargo.toml" | "Cargo.lock" => return Some(Server::RUST_ANALYZER),
        "go.mod" | "go.sum" | "go.work" | "go.work.sum" => return Some(Server::GOPLS),
        _ => {}
    }
    cide_lsp::discover::servers()
        .into_iter()
        .find(|server| server.def().project_markers.iter().any(|m| m == name))
}

/// Drain the handles, fold into the store, emit — coalesced. Pay what a disk change owes.
#[allow(clippy::too_many_arguments)]
fn pump(
    app: tauri::AppHandle,
    project: ProjectId,
    roots: Vec<PathBuf>,
    store: Arc<Mutex<DiagnosticStore>>,
    handles: Arc<Mutex<Vec<LspHandle>>>,
    stop: Arc<AtomicBool>,
    kick: Arc<Kick>,
    open_docs: OpenDocuments,
) {
    // The two halves of the debounce: when the first un-emitted change arrived, and when the last
    // one did. See the module docs for why both are needed.
    let mut first_dirty: Option<Instant> = None;
    let mut last_change: Option<Instant> = None;
    // When the memory watchdog last read /proc. Starts "just ran" so a project open is not
    // immediately followed by a check of servers that have not indexed anything yet.
    let mut last_watchdog = Instant::now();

    while !stop.load(Ordering::Acquire) {
        let mut changed = false;
        {
            // The handles lock is held only while draining, never across the emit: `restart`
            // takes it from a command thread, and an emit reaches the webview.
            let handles = handles.lock();
            for handle in handles.iter() {
                let source = handle.server().source();
                for event in handle.drain() {
                    match event {
                        // A new life shook hands: tell it about every open document it owns.
                        // (M59) See [`OpenDocuments`] for why this is the app's job. `send` is a
                        // `try_send`, so holding both locks across the loop blocks nothing;
                        // nothing in the store moved, so this is not a `changed`.
                        LspEvent::Handshook => {
                            let server = handle.server();
                            let (session, _) = cide_lsp::Session::new(&roots, server);
                            let docs = open_docs.lock();
                            for (path, doc) in
                                owned_documents(&docs, |path| server_for(path) == Some(server))
                            {
                                let Some(language) = language_of(path) else {
                                    continue;
                                };
                                let uri = cide_lsp::convert::path_to_uri(path);
                                if let cide_lsp::Effect::Send(value) = session.did_open(
                                    uri,
                                    &server.language_id_for(Some(&language)),
                                    doc.version,
                                    doc.text.clone(),
                                ) {
                                    handle.send(value);
                                }
                            }
                            tracing::debug!(
                                project = ?project,
                                source = %source,
                                "replayed the open documents to a new life"
                            );
                        }
                        LspEvent::Status(status) => {
                            changed = true;
                            // The one line that says what the panel was told, because the
                            // panel is the end of a four-hop pipe (server → session → store
                            // → webview) and "the bar shows X but the label says Y" has
                            // already cost one evening of guessing at which hop lied.
                            tracing::debug!(
                                project = ?project,
                                source = %source,
                                status = ?status,
                                "diagnostics status"
                            );
                            store.lock().set_status(source.clone(), status)
                        }
                        LspEvent::Published { abs_path, items } => {
                            changed = true;
                            let rel = relative(&roots, &abs_path);
                            let converted = items
                                .iter()
                                .map(|item| {
                                    cide_lsp::convert::diagnostic(
                                        item,
                                        &abs_path,
                                        &rel,
                                        &handle.server().binary(),
                                    )
                                })
                                .collect();
                            store.lock().publish(source.clone(), abs_path, converted);
                        }
                    }
                }
            }
        }

        /*
         * A disk change moved the staleness marks, so the snapshot the panel reads has changed
         * even though no server said a word. Folded into the *emit* debounce and not the kick's:
         * the mark is what tells the user "this row may point at the wrong line", and it is worth
         * nothing if it appears only after the re-check it is warning about has already finished.
         */
        if kick.take_marked() {
            changed = true;
        }

        /*
         * And the expensive half, once the burst has settled. Taken here rather than in
         * `files_changed` because that runs on the watcher thread: a `cargo fmt` over four hundred
         * files is four hundred events inside a second, and four hundred `cargo check` runs is a
         * machine that never recovers. See `KICK_DEBOUNCE`.
         *
         * gopls is deliberately absent from this branch. It was told at once, in `files_changed`,
         * because `workspace/didChangeWatchedFiles` is how it learns anything at all and it does
         * its own throttling; rust-analyzer's kick is a whole-workspace `cargo check` and is the
         * only one that has to be rationed.
         */
        if kick.take_due() {
            let handles = handles.lock();
            for handle in handles.iter() {
                if handle.server() != Server::RUST_ANALYZER {
                    continue;
                }
                let (session, _) = cide_lsp::Session::new(&roots, Server::RUST_ANALYZER);
                if let cide_lsp::Effect::Send(value) = session.run_flycheck() {
                    handle.send(value);
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

        /*
         * The memory watchdog. (M25, phase 2 of ADR 0011)
         *
         * Every WATCHDOG_EVERY, read each live server's resident memory from /proc and
         * respawn any that crossed the user's limit. In the pump rather than a thread of its
         * own because everything a respawn needs already lives here, and because this thread
         * already owns the discipline the handles lock demands — collected under the lock,
         * acted on after it, exactly like the drain above, since `respawn` takes the same
         * lock itself.
         *
         * The limit is read fresh from settings on every check, so flipping the setting
         * needs no plumbing and no restart. Zero — the shipped default — costs one atomic
         * load and a map read per interval. A watchdog respawn deliberately does NOT count
         * against the supervisor's three-crashes-in-five-minutes budget: a fresh supervisor
         * starts with a fresh budget by construction. What bounds a *thrashing* limit (a
         * workspace whose index simply needs more than the user allowed) is the interval
         * itself: at worst one re-index per WATCHDOG_EVERY, visible in the panel as the
         * server re-scanning, with the memory genuinely released in between. The disk index
         * (phase 1) is what turns that worst case from a re-index into a warm load.
         */
        if last_watchdog.elapsed() >= WATCHDOG_EVERY {
            last_watchdog = now;
            let limit_mb = watchdog_limit_mb(&app);
            if limit_mb > 0 {
                let over: Vec<(Server, u64)> = {
                    let handles = handles.lock();
                    handles
                        .iter()
                        .filter_map(|handle| {
                            let rss = resident_memory_bytes(handle.pid()?)?;
                            (rss > u64::from(limit_mb) * 1024 * 1024)
                                .then_some((handle.server(), rss))
                        })
                        .collect()
                };
                for (server, rss) in over {
                    tracing::warn!(
                        server = server.binary(),
                        resident_mib = rss / (1024 * 1024),
                        limit_mib = limit_mb,
                        "over the memory limit; restarting",
                    );
                    respawn(&app, project, &store, &handles, &roots, server);
                }
            }
        }

        std::thread::sleep(TICK);
    }
}

/// How often the watchdog reads `/proc`. Frequent enough that a runaway indexer is caught
/// inside a minute, rare enough that the cost rounds to zero against the 100 ms tick.
const WATCHDOG_EVERY: Duration = Duration::from_secs(10);

/// The user's memory limit, in MiB, `0` for off — read fresh so a settings change needs no
/// plumbing. Absent state (tests, early startup) reads as off, which is also the default.
fn watchdog_limit_mb(app: &tauri::AppHandle) -> u32 {
    app.try_state::<crate::workspace_state::WorkspaceState>()
        .map(|state| state.with(|ws| ws.settings.inspections.server_memory_limit_mb))
        .unwrap_or(0)
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
            stale: false,
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
            DiagnosticSourceId::rust_analyzer(),
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
            DiagnosticSourceId::rust_analyzer(),
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
        let missing = Missing::NotRunning(Server::GOPLS);
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
    fn a_build_manifest_belongs_to_a_server_even_though_no_buffer_opens_it() {
        // `server_for` answers "who do I send a didOpen for this buffer to", and a `Cargo.toml`
        // has no `languageId`. `owner_of` answers "whose view of the world does a write here
        // invalidate", which is strictly larger — editing `go.mod` changes what gopls resolves,
        // and a diagnostic in some `.go` file may now be wrong.
        use std::path::Path;
        assert_eq!(
            owner_of(Path::new("/r/Cargo.toml")),
            Some(Server::RUST_ANALYZER)
        );
        assert_eq!(
            owner_of(Path::new("/r/Cargo.lock")),
            Some(Server::RUST_ANALYZER)
        );
        assert_eq!(owner_of(Path::new("/r/go.mod")), Some(Server::GOPLS));
        assert_eq!(owner_of(Path::new("/r/go.work")), Some(Server::GOPLS));
        assert_eq!(
            owner_of(Path::new("/r/src/a.rs")),
            Some(Server::RUST_ANALYZER)
        );
        assert_eq!(owner_of(Path::new("/r/a.go")), Some(Server::GOPLS));
        // And nothing else. A `.gitignore` or a `README.md` moves nothing either server has said,
        // and treating it as a change would start a `cargo check` every time a note was saved.
        assert_eq!(owner_of(Path::new("/r/.gitignore")), None);
        assert_eq!(owner_of(Path::new("/r/README.md")), None);
        // `server_for` stays narrow, which is the point of having two functions.
        assert_eq!(server_for(Path::new("/r/Cargo.toml")), None);
    }

    #[test]
    fn a_burst_of_disk_changes_produces_one_flycheck_and_not_one_each() {
        // The correctness requirement the `Kick` type exists for. A `cargo fmt` over four hundred
        // files is four hundred watcher events inside a second, and four hundred whole-workspace
        // `cargo check` runs is a machine that never recovers — the same argument the emit
        // coalescer above makes, one order of magnitude up.
        let kick = Kick::default();
        for _ in 0..400 {
            kick.schedule(false);
        }
        assert!(
            !kick.take_due(),
            "the kick fired while the burst was still arriving"
        );

        // Nothing has arrived for longer than the trailing debounce: now it is due, exactly once.
        {
            let mut state = kick.state.lock();
            state.last = Some(Instant::now() - KICK_DEBOUNCE - Duration::from_millis(10));
        }
        assert!(kick.take_due(), "a settled burst never became due");
        assert!(!kick.take_due(), "one burst produced two kicks");
    }

    #[test]
    fn a_burst_that_never_settles_still_gets_re_checked() {
        // The failure a bare trailing debounce always has. `cargo watch` in a terminal pane, or
        // any tool that touches a file every few hundred milliseconds, would reset `last` for ever
        // and the kick would never fire at all.
        let kick = Kick::default();
        kick.schedule(false);
        {
            let mut state = kick.state.lock();
            state.first = Some(Instant::now() - KICK_CEILING - Duration::from_millis(10));
            // Still arriving: the trailing debounce alone would say "not yet".
            state.last = Some(Instant::now());
        }
        assert!(
            kick.take_due(),
            "the ceiling did not override the trailing debounce"
        );
    }

    #[test]
    fn nothing_owed_is_never_due() {
        // A pump that fired on an empty kick would send `runFlycheck` every 100 ms for the life of
        // the project, which is a `cargo check` loop nobody asked for.
        let kick = Kick::default();
        assert!(!kick.take_due());
        assert!(!kick.take_marked());
    }

    #[test]
    fn the_panels_marks_are_due_immediately_and_the_flycheck_is_not() {
        // Two different clocks on purpose. The stale mark is worth nothing if it appears only
        // after the re-check it is warning about has already finished, so it rides the emit
        // coalescer (250 ms) rather than the kick's debounce (500 ms) and ceiling (5 s).
        let kick = Kick::default();
        kick.schedule(true);
        assert!(
            kick.take_marked(),
            "the panel had to wait out the kick debounce"
        );
        assert!(!kick.take_marked(), "the flag was not consumed");
        assert!(!kick.take_due(), "the expensive half fired at once");
    }

    #[test]
    fn a_manual_rerun_cancels_what_the_watcher_had_parked() {
        // Otherwise the button's kick is followed a moment later by the burst's kick, which is
        // two `cargo check` runs for one gesture.
        let kick = Kick::default();
        kick.schedule(true);
        kick.clear();
        assert!(!kick.take_marked());
        assert!(!kick.take_due());
    }

    #[test]
    fn the_kick_is_rationed_more_heavily_than_the_emit() {
        // Both are debounces and they are not interchangeable: coalescing an emit too eagerly
        // costs a redundant serialization, kicking flycheck too eagerly costs a whole `cargo
        // check` of the workspace.
        assert!(KICK_DEBOUNCE > COALESCE);
        assert!(KICK_CEILING > COALESCE_CEILING);
        assert!(
            TICK < KICK_DEBOUNCE,
            "the pump cannot notice its own debounce"
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

    /// The replay after a new life tells a server about the documents *it* owns and no others —
    /// a Go file replayed to rust-analyzer is a `didOpen` for a language it never declared. The
    /// ownership question is passed in because the registry is process-global. (M59)
    #[test]
    fn a_new_life_is_told_about_the_documents_it_owns_and_no_others() {
        let mut docs: BTreeMap<PathBuf, OpenDocument> = BTreeMap::new();
        for (path, text) in [
            ("/p/a.gd", "extends Node"),
            ("/p/b.rs", "fn a() {}"),
            ("/p/c.gd", ""),
        ] {
            docs.insert(
                PathBuf::from(path),
                OpenDocument {
                    version: 3,
                    text: text.into(),
                },
            );
        }
        let owned = owned_documents(&docs, |path| {
            path.extension().is_some_and(|ext| ext == "gd")
        });
        let paths: Vec<&str> = owned
            .iter()
            .map(|(path, _)| path.to_str().unwrap_or_default())
            .collect();
        assert_eq!(paths, vec!["/p/a.gd", "/p/c.gd"]);
        assert_eq!(
            owned[0].1,
            &OpenDocument {
                version: 3,
                text: "extends Node".into()
            },
            "with the last text and version the editor sent, so the server sees the buffer as \
             it is and not as the disk has it"
        );
        assert!(owned_documents(&docs, |_| false).is_empty());
    }
}
