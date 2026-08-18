//! One conversation with one language server, as a pure state machine.
//!
//! Messages in, [`Effect`]s out. No I/O, no threads, no process — the same shape
//! `cide_claude::state::next_state` and `cide_app::hooks::decide` already use, and for the same
//! reason: everything below has a failure mode that looks like "the server is slow", and none of
//! them is reproducible against a real rust-analyzer on demand.
//!
//! # The two requests that must be answered or rust-analyzer never starts
//!
//! A server may send *requests* to the client, not just notifications, and it **blocks** on them.
//! Two arrive during initialization:
//!
//! * `workspace/configuration` — rust-analyzer asks for its settings and waits. A client that
//!   ignores unknown server→client requests leaves it there for ever.
//! * `window/workDoneProgress/create` — sent before the first `$/progress`. Same trap.
//!
//! Both look exactly like a slow index, which is why each has its own named test below. The
//! general rule follows from them and is worth stating on its own: **an unknown request is
//! answered with an error, never dropped.** Dropping it is indistinguishable from a hang.
//!
//! # When a source becomes `Ready`
//!
//! When the handshake has completed **and** no work-done progress token is in flight — never
//! "when a diagnostic has been seen". A clean Rust workspace publishes nothing at all, so a
//! ready-requires-a-diagnostic rule would leave it `Scanning` for ever: the mirror image of the
//! failure `cide_core::diagnostics` exists to prevent, and just as much of a lie.

use std::collections::BTreeSet;

use cide_ipc::SourceStatus;
use serde_json::{Value, json};

use crate::discover::Server;

/// What the session wants done as a result of a message.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Write this back to the server.
    Send(Value),
    /// Replace one file's diagnostics for this source.
    Publish {
        abs_path: String,
        items: Vec<lsp_types::Diagnostic>,
    },
    /// The source's status changed.
    Status(SourceStatus),
}

/// Where the handshake has got to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// `initialize` sent, waiting for its result.
    Initializing,
    /// `initialized` sent. The server may now be asked for anything.
    Running,
    /// `shutdown` sent.
    Stopping,
}

pub struct Session {
    phase: Phase,
    /// Work-done tokens the server has begun and not ended.
    ///
    /// A set rather than a counter: `$/progress` `end` names its token, and a server that ends one
    /// twice — or ends one it never began, which rust-analyzer has done across versions — would
    /// drive a counter negative and leave the source `Ready` while it was still indexing.
    progress: BTreeSet<String>,
    /// The most recent progress line, for [`SourceStatus::Scanning::detail`].
    detail: String,
    initialize_id: i64,
    next_id: i64,
    /// The last status handed out, so a redundant one is not re-emitted on every message.
    last_status: Option<SourceStatus>,
    /// `result.capabilities` from the handshake, verbatim.
    ///
    /// Kept because the alternative to asking is *finding out by timing out*: a server with no
    /// `referencesProvider` answers `MethodNotFound` at best and nothing at all at worst, and a
    /// twenty-second wait that ends in "it is probably still indexing" is a lie about a feature
    /// that was never going to arrive. The whole value rather than a handful of booleans, so the
    /// next question anybody asks of it costs a reader and not a field.
    capabilities: Value,
}

impl Session {
    /// A session that has just sent `initialize`.
    ///
    /// Returns the request to write, so the caller never has to know the handshake's shape.
    pub fn new(roots: &[std::path::PathBuf], server: Server) -> (Self, Vec<Effect>) {
        let mut session = Self {
            phase: Phase::Initializing,
            progress: BTreeSet::new(),
            detail: format!("starting {}", server.binary()),
            initialize_id: 1,
            next_id: 2,
            last_status: None,
            capabilities: Value::Null,
        };
        let request = json!({
            "jsonrpc": "2.0",
            "id": session.initialize_id,
            "method": "initialize",
            "params": initialize_params(roots, server),
        });
        let effects = vec![
            Effect::Send(request),
            Effect::Status(SourceStatus::Scanning {
                detail: session.detail.clone(),
            }),
        ];
        session.last_status = Some(SourceStatus::Scanning {
            detail: session.detail.clone(),
        });
        (session, effects)
    }

    /// Consume one inbound message.
    pub fn on_message(&mut self, message: &Value) -> Vec<Effect> {
        let mut effects = Vec::new();

        // A *response* — it has an `id` and no `method`.
        if message.get("method").is_none() {
            if message.get("id").and_then(Value::as_i64) == Some(self.initialize_id)
                && self.phase == Phase::Initializing
            {
                self.phase = Phase::Running;
                // The one moment this arrives. `initialize` is answered once per life of the
                // server and the result is never repeated, so a client that does not read it here
                // cannot read it at all — which is why `capabilities` was `Value::Null` for
                // everybody until Find usages needed to know whether asking was worth a wait.
                self.capabilities = message
                    .get("result")
                    .and_then(|result| result.get("capabilities"))
                    .cloned()
                    .unwrap_or(Value::Null);
                effects.push(Effect::Send(json!({
                    "jsonrpc": "2.0",
                    "method": "initialized",
                    "params": {},
                })));
                self.push_status(&mut effects);
            }
            return effects;
        }

        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        match (method, &id) {
            // --- server → client *requests*: every one must be answered ---
            ("workspace/configuration", Some(id)) => {
                // One `{}` per requested item — server defaults. Answering with the wrong *shape*
                // (a bare object rather than an array) is as bad as not answering: rust-analyzer
                // rejects it and stalls the same way.
                let count = params
                    .get("items")
                    .and_then(Value::as_array)
                    .map_or(1, Vec::len);
                let result: Vec<Value> = (0..count).map(|_| json!({})).collect();
                effects.push(Effect::Send(response(id.clone(), json!(result))));
            }
            ("window/workDoneProgress/create", Some(id)) => {
                if let Some(token) = params.get("token").and_then(token_string) {
                    self.progress.insert(token);
                }
                effects.push(Effect::Send(response(id.clone(), Value::Null)));
                self.push_status(&mut effects);
            }
            ("client/registerCapability", Some(id)) | ("client/unregisterCapability", Some(id)) => {
                /*
                 * Accepted and ignored, and since M18 the *ignored* half is load-bearing enough
                 * to spell out.
                 *
                 * gopls is told `workspace.didChangeWatchedFiles.dynamicRegistration: true` (see
                 * `declares_watched_files`), so it registers watch globs through this request. We
                 * answer `null` and keep no record of what it asked for: the notifications cide
                 * sends are chosen by `cide_app::lsp::owner_of` — every `.go`, `go.mod`, `go.sum`
                 * and `go.work` the file watcher reports under the project's roots — rather than
                 * by matching the server's patterns. That is deliberate. Honouring the globs would
                 * mean a second path-matching implementation whose only proof of correctness is
                 * that a server did not complain, and the set cide sends is already narrower than
                 * anything gopls registers, so honouring them could only ever *drop* an event.
                 * `gopls_re_diagnoses_a_file_it_was_told_changed_on_disk` is the standing proof
                 * that an unregistered-but-sent notification is acted on.
                 *
                 * Answering at all is not optional: refusing makes gopls log an error on every
                 * start, and refusing *by silence* would hang it — a request is never dropped.
                 */
                effects.push(Effect::Send(response(id.clone(), Value::Null)));
            }
            (_, Some(id)) => {
                // The rule: an unknown *request* is answered with an error, never dropped. A
                // dropped request is indistinguishable from a hung client, and the server is
                // blocked on it.
                effects.push(Effect::Send(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": format!("cide does not implement {method}") },
                })));
            }

            // --- notifications: no reply, ever ---
            ("textDocument/publishDiagnostics", None) => {
                if let Some(effect) = publish(&params) {
                    effects.push(effect);
                }
            }
            ("$/progress", None) => {
                self.on_progress(&params);
                self.push_status(&mut effects);
            }
            ("window/logMessage", None) | ("window/showMessage", None) => {
                if let Some(text) = params.get("message").and_then(Value::as_str) {
                    tracing::debug!(target: "cide::lsp", %text, "server message");
                }
            }
            _ => {}
        }

        effects
    }

    fn on_progress(&mut self, params: &Value) {
        let Some(token) = params.get("token").and_then(token_string) else {
            return;
        };
        let value = params.get("value");
        let kind = value
            .and_then(|v| v.get("kind"))
            .and_then(Value::as_str)
            .unwrap_or("");
        match kind {
            "begin" | "report" => {
                self.progress.insert(token);
                // `title` on begin, `message` on report. Both are what the panel shows instead of
                // an unqualified "Waiting for rust-analyzer" — which, for the two minutes a large
                // workspace takes, is indistinguishable from a hang.
                let title = value
                    .and_then(|v| v.get("title"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let message = value
                    .and_then(|v| v.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let detail = [title, message]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(" — ");
                if !detail.is_empty() {
                    self.detail = detail;
                }
            }
            "end" => {
                self.progress.remove(&token);
            }
            _ => {}
        }
    }

    /// The status this session is in right now, emitted only when it changed.
    fn push_status(&mut self, effects: &mut Vec<Effect>) {
        let status = self.status();
        if self.last_status.as_ref() == Some(&status) {
            return;
        }
        self.last_status = Some(status.clone());
        effects.push(Effect::Status(status));
    }

    /// Has the handshake ever completed?
    ///
    /// The supervisor's fatal-versus-transient test. A server that dies *before* answering
    /// `initialize` has not failed at its job — it has failed to start, which no amount of
    /// retrying fixes. See `server::supervise`.
    pub fn ever_handshook(&self) -> bool {
        self.phase != Phase::Initializing
    }

    /// Does this server answer `textDocument/references`?
    ///
    /// `referencesProvider` is `boolean | ReferenceOptions` in the spec — an object being the shape
    /// a server uses when it wants to attach work-done-progress support — so **both** count, and
    /// reading only the boolean would report "gopls cannot find usages" on any release that
    /// switched to the object form.
    ///
    /// `false` before the handshake completes, which is the honest answer: nothing has said yet.
    /// The caller must not turn that into a refusal — see `LspHandle::supports_references`, which
    /// is where the racing-against-startup case is decided.
    pub fn supports_references(&self) -> bool {
        match self.capabilities.get("referencesProvider") {
            Some(Value::Bool(yes)) => *yes,
            Some(Value::Object(_)) => true,
            _ => false,
        }
    }

    /// Does this server answer `textDocument/implementation`?
    ///
    /// The same `boolean | Options` reading as [`Self::supports_references`], and for the same
    /// reason — `implementationProvider` is `boolean | ImplementationOptions |
    /// ImplementationRegistrationOptions` in the spec, and reading only the boolean would report
    /// "gopls cannot do this" about the server whose answer is the entire point of the feature.
    ///
    /// `false` before the handshake completes is "nothing has said yet", not a refusal. See
    /// `LspHandle::supports_implementation`, which is where that distinction is kept.
    pub fn supports_implementation(&self) -> bool {
        match self.capabilities.get("implementationProvider") {
            Some(Value::Bool(yes)) => *yes,
            Some(Value::Object(_)) => true,
            _ => false,
        }
    }

    fn status(&self) -> SourceStatus {
        // Ready needs *both*: the handshake done, and nothing in flight. See the module docs for
        // why "a diagnostic has arrived" is not part of it.
        if self.phase == Phase::Running && self.progress.is_empty() {
            SourceStatus::Ready
        } else {
            SourceStatus::Scanning {
                detail: self.detail.clone(),
            }
        }
    }

    /// `textDocument/didOpen`.
    pub fn did_open(&self, uri: String, language_id: &str, version: i32, text: String) -> Effect {
        Effect::Send(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": { "textDocument": {
                "uri": uri, "languageId": language_id, "version": version, "text": text,
            }},
        }))
    }

    /// `textDocument/didChange`, full-text.
    ///
    /// Full and not incremental: incremental means translating CodeMirror `ChangeSet`s into LSP
    /// ranges — a second serialization format, a second source of off-by-one, and a bug class
    /// where the server's view of the file silently diverges from the user's with nothing to
    /// notice it. Debounced by the caller, never per keystroke.
    pub fn did_change(&self, uri: String, version: i32, text: String) -> Effect {
        Effect::Send(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [ { "text": text } ],
            },
        }))
    }

    pub fn did_save(&self, uri: String) -> Effect {
        Effect::Send(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didSave",
            "params": { "textDocument": { "uri": uri } },
        }))
    }

    pub fn did_close(&self, uri: String) -> Effect {
        Effect::Send(json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didClose",
            "params": { "textDocument": { "uri": uri } },
        }))
    }

    /// `workspace/didChangeWatchedFiles` — files changed on disk, by somebody other than us. (M18)
    ///
    /// # Why this exists at all
    ///
    /// cide's whole premise is that an agent edits the tree while the user watches. Those writes
    /// arrive through `notify`, land in `cide_app::files::on_watch_event`, and until M18 stopped
    /// there: the language servers were told about a file **only** through `didOpen`/`didChange`
    /// for a buffer the user had open. So a file Claude fixed and the user never opened kept its
    /// diagnostics, with their original line numbers, for the rest of the session — which is the
    /// report this notification answers.
    ///
    /// `changes` is `(uri, kind)` where kind is LSP's `FileChangeType`: `1` created, `2` changed,
    /// `3` deleted. Numbers rather than an enum because they cross straight to JSON and this crate
    /// deliberately re-exports nothing from `lsp-types`.
    ///
    /// A notification, so a server that does not implement it drops it in silence — which is the
    /// property that makes sending it unconditionally safe.
    pub fn did_change_watched_files(&self, changes: Vec<(String, u8)>) -> Effect {
        let changes: Vec<Value> = changes
            .into_iter()
            .map(|(uri, kind)| json!({ "uri": uri, "type": kind }))
            .collect();
        Effect::Send(json!({
            "jsonrpc": "2.0",
            "method": "workspace/didChangeWatchedFiles",
            "params": { "changes": changes },
        }))
    }

    /// `rust-analyzer/runFlycheck` — re-run `cargo check`. (M18)
    ///
    /// # Why a vendor extension is the right answer here and `didSave` is not
    ///
    /// rust-analyzer's semantic analysis follows the disk on its own (it runs its own `notify`
    /// watcher whenever the client does not claim `didChangeWatchedFiles`), but **flycheck** —
    /// the `cargo check` that produces every `E0308`, every `unused_variables`, and essentially
    /// everything a user calls "the errors" — runs only on a save. `crates/cide-lsp/tests/
    /// real_servers.rs::an_on_disk_edit_alone_never_refreshes_diagnostics` is the standing proof:
    /// it repairs a broken file on disk, waits two minutes and asserts the diagnostic is *still
    /// there*.
    ///
    /// The alternative was to synthesise a `textDocument/didSave` for a document nobody has open.
    /// It is a lie about the client's state, rust-analyzer logs an orphan save when the document
    /// is not in its `mem_docs`, and it fights the refcounting `ui/src/editor/docSync.ts` keeps.
    /// Asking the server to do the thing we want, by its own name for it, costs one notification.
    ///
    /// `textDocument: null` means "the whole workspace", which is what a watcher burst means.
    /// A notification, and an extension — so any server that has never heard of it, including a
    /// future replacement for rust-analyzer, drops it silently rather than erroring.
    pub fn run_flycheck(&self) -> Effect {
        Effect::Send(json!({
            "jsonrpc": "2.0",
            "method": "rust-analyzer/runFlycheck",
            "params": { "textDocument": null },
        }))
    }

    /// The orderly half of the shutdown ladder: `shutdown` (a request), then `exit`.
    ///
    /// Both, in this order, before any signal. gopls writes its cache on `exit`, and a `SIGTERM`
    /// first would cost the user that cache — and the next start would re-index from nothing.
    pub fn shutdown(&mut self) -> Vec<Effect> {
        self.phase = Phase::Stopping;
        let id = self.next_id;
        self.next_id += 1;
        vec![
            Effect::Send(json!({ "jsonrpc": "2.0", "id": id, "method": "shutdown" })),
            Effect::Send(json!({ "jsonrpc": "2.0", "method": "exit" })),
        ]
    }
}

fn response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// A progress token is `string | integer`. Both are keys, so both become strings.
fn token_string(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn publish(params: &Value) -> Option<Effect> {
    let uri = params.get("uri").and_then(Value::as_str)?;
    let abs_path = crate::convert::uri_to_path(uri)?;
    // An unparseable diagnostic array is dropped whole rather than partially: half a file's
    // diagnostics is a worse answer than the previous complete one.
    let items: Vec<lsp_types::Diagnostic> =
        serde_json::from_value(params.get("diagnostics").cloned()?).ok()?;
    Some(Effect::Publish {
        abs_path: abs_path.to_string_lossy().into_owned(),
        items,
    })
}

/// What cide tells a server it can do.
///
/// Every line here is load-bearing:
/// * `positionEncodings: ["utf-16"]` — the only encoding every server supports, and what
///   `convert` assumes.
/// * `window.workDoneProgress: true` — **without it rust-analyzer never sends `$/progress`**, and
///   the panel has nothing to show for the two minutes it indexes.
/// * `workspace.configuration: true` — we answer that request, so we may as well say so.
/// * `publishDiagnostics.*Support` — asking for the fields `convert` reads.
///
/// # Why this takes the `Server` and not a name (M18)
///
/// Because exactly one capability must differ between them, and getting that one wrong trades a
/// stale-diagnostics bug for a wrong-answer bug, which is worse. See
/// [`declares_watched_files`].
fn initialize_params(roots: &[std::path::PathBuf], server: Server) -> Value {
    let folders: Vec<Value> = roots
        .iter()
        .map(|root| {
            json!({
                "uri": crate::convert::path_to_uri(root),
                "name": root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            })
        })
        .collect();
    json!({
        "processId": std::process::id(),
        "clientInfo": { "name": "cide", "version": env!("CARGO_PKG_VERSION") },
        "rootUri": roots.first().map(|r| crate::convert::path_to_uri(r)),
        "workspaceFolders": folders,
        "capabilities": {
            "general": { "positionEncodings": ["utf-16"] },
            "window": { "workDoneProgress": true },
            "workspace": workspace_capabilities(server),
            "textDocument": {
                "synchronization": {
                    "didSave": true,
                    "willSave": false,
                    "willSaveWaitUntil": false,
                },
                "publishDiagnostics": {
                    "relatedInformation": true,
                    "versionSupport": true,
                    "codeDescriptionSupport": true,
                    "dataSupport": true,
                    "tagSupport": { "valueSet": [1, 2] },
                },
                // `linkSupport: false` is a statement about the *reply shape*, not a missing
                // feature. With it false a conforming server may answer only
                // `Location | Location[] | null`; flipping it to true additionally permits
                // `LocationLink[]`, whose target lives under `targetSelectionRange` instead of
                // `range` — and `convert::location` would then silently find no range and jump to
                // line 1. Declared explicitly, and pinned by the handshake test, so that turning it
                // on has to be a deliberate edit in two places rather than a one-word change here.
                "definition": { "linkSupport": false },
                /*
                 * Find usages. (M14)
                 *
                 * Note the asymmetry with the line above, because it is the one thing that makes
                 * this reply *simpler* than definition's: `textDocument/references` has **no
                 * `linkSupport` analogue**. Its result is `Location[] | null` and nothing else, so
                 * the `LocationLink` hazard `convert::location` guards against cannot arise here
                 * and `convert::locations` needs no equivalent refusal.
                 *
                 * `dynamicRegistration: false` is stated rather than omitted, for the same reason
                 * `linkSupport` is: an absent capability and a capability declared false are the
                 * same thing to a server and very different things to the next person reading
                 * this list.
                 */
                "references": { "dynamicRegistration": false },
                /*
                 * Go to implementation. (M18)
                 *
                 * The user's report was Go-shaped — *"goto for golang goes to the interface
                 * declaration, but should go to the implementation"* — and gopls is behaving
                 * correctly there: `textDocument/definition` on a call through an interface
                 * resolves to the interface's method, because that *is* where the thing being
                 * called is declared. The question the user is asking is a different protocol
                 * request, and cide never asked it.
                 *
                 * `linkSupport: false` for exactly the reason spelled out over `definition`
                 * above, and it matters more here rather than less: `textDocument/implementation`
                 * has the same `Location | Location[] | LocationLink[] | null` result shape, and
                 * `convert::locations` reads `range` — so a `LocationLink[]` reply would parse
                 * into rows that all point at line 1. Declared false, pinned by the handshake
                 * test, so turning it on has to be a deliberate edit in two places.
                 *
                 * `dynamicRegistration: false` stated rather than omitted, for the reason the
                 * `references` block gives.
                 */
                "implementation": { "dynamicRegistration": false, "linkSupport": false },
            },
        },
    })
}

/// The `capabilities.workspace` object, which is the one part that differs per server.
///
/// Built rather than written as a literal so `didChangeWatchedFiles` can be **absent** and not
/// `null`. Both deserialize to `None` in every server this app ships, but "we did not say" and
/// "we said no" are different sentences on a wire, and the whole reason this function exists is
/// that a server reads this key as a statement about who is responsible for watching the disk.
fn workspace_capabilities(server: Server) -> Value {
    let mut workspace = json!({
        "workspaceFolders": true,
        "configuration": true,
    });
    if declares_watched_files(server) {
        workspace["didChangeWatchedFiles"] = json!({ "dynamicRegistration": true });
    }
    workspace
}

/// Does this server get told that **cide** watches the files? (M18)
///
/// `true` for gopls, `false` for rust-analyzer, and the asymmetry is the whole point.
///
/// `workspace.didChangeWatchedFiles` is not a request for a feature — it is a **transfer of
/// responsibility**. A server that sees it stops watching the disk itself and waits for the
/// client to tell it what changed.
///
/// * **gopls has no watcher of its own.** It registers watch patterns through
///   `client/registerCapability` and relies entirely on `workspace/didChangeWatchedFiles` for
///   everything the editor did not tell it about. Without the capability declared it is never
///   told about an out-of-editor write at all — which is the Go half of the stale-diagnostics
///   report.
/// * **rust-analyzer has one, and it sees more than cide's does.** Declaring this for it would
///   make cide solely responsible for file notifications, and cide's watcher is gitignore-filtered
///   and confined to the project's own roots (`cide_fs::watch`). rust-analyzer additionally
///   watches path dependencies and `~/.cargo/registry` sources; handing it a narrower feed would
///   quietly break analysis of exactly the code a user cannot see to suspect. Its stale-diagnostic
///   problem is a *flycheck* problem, not a file-notification one, and [`Session::run_flycheck`]
///   is the answer to that.
fn declares_watched_files(server: Server) -> bool {
    match server {
        Server::Gopls => true,
        Server::RustAnalyzer => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started() -> Session {
        let (session, _) = Session::new(&[std::path::PathBuf::from("/repo")], Server::RustAnalyzer);
        session
    }

    /// Drive the handshake to completion, discarding the effects.
    fn running() -> Session {
        let mut session = started();
        session.on_message(&json!({"jsonrpc": "2.0", "id": 1, "result": {"capabilities": {}}}));
        session
    }

    fn sent(effects: &[Effect]) -> Vec<Value> {
        effects
            .iter()
            .filter_map(|e| match e {
                Effect::Send(v) => Some(v.clone()),
                _ => None,
            })
            .collect()
    }

    fn status(effects: &[Effect]) -> Option<SourceStatus> {
        effects.iter().find_map(|e| match e {
            Effect::Status(s) => Some(s.clone()),
            _ => None,
        })
    }

    #[test]
    fn the_handshake_declares_what_the_client_actually_does() {
        let (_, effects) = Session::new(&[std::path::PathBuf::from("/repo")], Server::RustAnalyzer);
        let request = &sent(&effects)[0];
        let caps = &request["params"]["capabilities"];
        // Without this rust-analyzer never sends `$/progress`, and the panel shows an
        // unqualified "waiting" for the whole of a two-minute index.
        assert_eq!(caps["window"]["workDoneProgress"], json!(true));
        // `convert` assumes it, and it is the only encoding every server supports.
        assert_eq!(caps["general"]["positionEncodings"], json!(["utf-16"]));
        // See the comment beside this capability: `true` here would let a server reply with
        // `LocationLink[]`, which `convert::location` does not read, and the symptom would be a
        // Go to Definition that always lands on line 1 rather than an error anybody can see.
        assert_eq!(
            caps["textDocument"]["definition"]["linkSupport"],
            json!(false)
        );
        // Find usages asks for nothing beyond existing, and that is the point: `references` has no
        // `linkSupport` analogue, so the reply shape is fixed at `Location[] | null` and
        // `convert::locations` needs no refusal branch. Declaring the capability at all is what
        // makes a server that gates the method behind client support answer it.
        assert_eq!(
            caps["textDocument"]["references"]["dynamicRegistration"],
            json!(false)
        );
        // Go to implementation, with the same `linkSupport: false` refusal as `definition` — a
        // `LocationLink[]` reply would parse into rows that all point at line 1.
        assert_eq!(
            caps["textDocument"]["implementation"]["linkSupport"],
            json!(false)
        );
        // We answer `workspace/configuration`, so we must say we can.
        assert_eq!(caps["workspace"]["configuration"], json!(true));
        assert_eq!(
            request["params"]["workspaceFolders"][0]["uri"],
            "file:///repo"
        );
    }

    #[test]
    fn only_gopls_is_told_that_cide_watches_the_files() {
        // The one capability that differs per server, pinned in both directions because getting
        // it wrong is worse than the bug it fixes.
        //
        // Declaring `didChangeWatchedFiles` *transfers responsibility*: the server stops watching
        // the disk and waits to be told. gopls has no watcher of its own, so it needs this or it
        // never hears about an agent's write. rust-analyzer does have one, and it watches path
        // dependencies and registry sources that cide's gitignore-filtered, per-root watcher does
        // not — so declaring it there would narrow what that server sees, silently, in exactly
        // the code a user cannot see to suspect.
        let (_, gopls) = Session::new(&[std::path::PathBuf::from("/repo")], Server::Gopls);
        let gopls = &sent(&gopls)[0]["params"]["capabilities"]["workspace"];
        assert_eq!(
            gopls["didChangeWatchedFiles"]["dynamicRegistration"],
            json!(true)
        );

        let (_, ra) = Session::new(&[std::path::PathBuf::from("/repo")], Server::RustAnalyzer);
        let ra = &sent(&ra)[0]["params"]["capabilities"]["workspace"];
        assert!(
            ra.get("didChangeWatchedFiles").is_none(),
            "rust-analyzer was told cide watches the files: {ra}"
        );
    }

    #[test]
    fn the_servers_own_implementation_capability_survives_the_handshake() {
        // Same three-state reading as `referencesProvider`, and for the same reason: a
        // `MethodNotFound` that never arrives is a twenty-second wait ending in "probably still
        // indexing" about a feature that was never going to appear.
        let mut session = started();
        session.on_message(&json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "capabilities": { "implementationProvider": true } },
        }));
        assert!(session.supports_implementation());

        // `boolean | ImplementationOptions`: the object form is what a server sends when it wants
        // work-done progress on the method, and reading only the boolean would report "gopls
        // cannot do this" about the server the whole feature exists for.
        let mut options = started();
        options.on_message(&json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "capabilities": { "implementationProvider": { "workDoneProgress": true } } },
        }));
        assert!(options.supports_implementation());

        // And a server that offers nothing says so rather than being assumed able.
        let mut none = started();
        none.on_message(&json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "capabilities": { "definitionProvider": true } },
        }));
        assert!(!none.supports_implementation());
        assert!(!started().supports_implementation());
    }

    #[test]
    fn a_watched_file_change_carries_lsps_own_change_kinds() {
        // 1 created, 2 changed, 3 deleted. A wrong number here is not an error anywhere — the
        // server simply acts on the wrong event, and a "deleted" for a file that was written is
        // how a whole package's diagnostics disappear.
        let session = running();
        let Effect::Send(sent) = session.did_change_watched_files(vec![
            ("file:///repo/a.go".into(), 2),
            ("file:///repo/b.go".into(), 3),
        ]) else {
            panic!()
        };
        assert_eq!(sent["method"], "workspace/didChangeWatchedFiles");
        assert!(sent.get("id").is_none(), "a notification, never a request");
        assert_eq!(sent["params"]["changes"][0]["uri"], "file:///repo/a.go");
        assert_eq!(sent["params"]["changes"][0]["type"], 2);
        assert_eq!(sent["params"]["changes"][1]["type"], 3);
    }

    #[test]
    fn a_flycheck_kick_names_the_whole_workspace_and_expects_no_reply() {
        // `textDocument: null` is rust-analyzer's own spelling of "everything", which is what a
        // watcher burst means. A *request* here would block a caller on a server that has never
        // heard of the method; as a notification it is silently dropped by anything that has not.
        let session = running();
        let Effect::Send(sent) = session.run_flycheck() else {
            panic!()
        };
        assert_eq!(sent["method"], "rust-analyzer/runFlycheck");
        assert!(sent.get("id").is_none(), "a notification, never a request");
        assert_eq!(sent["params"]["textDocument"], Value::Null);
    }

    #[test]
    fn a_multi_root_project_advertises_all_of_its_roots() {
        let roots = [
            std::path::PathBuf::from("/repo/a"),
            std::path::PathBuf::from("/repo/b"),
        ];
        let (_, effects) = Session::new(&roots, Server::Gopls);
        let folders = sent(&effects)[0]["params"]["workspaceFolders"]
            .as_array()
            .expect("array")
            .len();
        assert_eq!(folders, 2);
    }

    #[test]
    fn the_initialize_result_is_answered_with_initialized() {
        let mut session = started();
        let effects =
            session.on_message(&json!({"jsonrpc": "2.0", "id": 1, "result": {"capabilities": {}}}));
        assert_eq!(sent(&effects)[0]["method"], "initialized");
    }

    #[test]
    fn the_servers_own_capabilities_survive_the_handshake() {
        // They were parsed and dropped on the floor for two milestones — `on_message` read the
        // result only to flip the phase — so there was no way for any caller to ask "does this
        // server do references?" at all. That is what turns a `MethodNotFound` into a
        // twenty-second wait ending in "it is probably still indexing".
        let mut session = started();
        session.on_message(&json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "capabilities": { "referencesProvider": true } },
        }));
        assert!(session.supports_references());
    }

    #[test]
    fn a_reference_provider_declared_as_options_still_counts() {
        // `referencesProvider` is `boolean | ReferenceOptions`. A server that wants work-done
        // progress on the method sends the object form, and reading only the boolean would report
        // "this server cannot find usages" about one that can.
        let mut session = started();
        session.on_message(&json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "capabilities": { "referencesProvider": { "workDoneProgress": true } } },
        }));
        assert!(session.supports_references());
    }

    #[test]
    fn a_server_that_offers_no_references_says_so_rather_than_being_assumed_able() {
        let mut session = started();
        session.on_message(&json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "capabilities": { "definitionProvider": true } },
        }));
        assert!(!session.supports_references());
        // And before the handshake there is nothing to read, which must be `false` and not a
        // panic. The caller decides what to do with "nobody has said yet" — see `LspHandle`.
        assert!(!started().supports_references());
    }

    #[test]
    fn workspace_configuration_is_answered_or_rust_analyzer_waits_forever() {
        // Not hypothetical, and the failure looks exactly like "rust-analyzer is slow".
        let mut session = running();
        let effects = session.on_message(&json!({
            "jsonrpc": "2.0", "id": 42, "method": "workspace/configuration",
            "params": { "items": [{"section": "rust-analyzer"}, {"section": "rust-analyzer.check"}] },
        }));
        let reply = &sent(&effects)[0];
        assert_eq!(reply["id"], 42);
        // An *array*, one entry per requested item. A bare object is rejected and stalls the
        // server exactly as silence would.
        assert_eq!(reply["result"], json!([{}, {}]));
    }

    #[test]
    fn a_work_done_progress_create_request_is_answered() {
        let mut session = running();
        let effects = session.on_message(&json!({
            "jsonrpc": "2.0", "id": 7, "method": "window/workDoneProgress/create",
            "params": { "token": "rustAnalyzer/Indexing" },
        }));
        assert_eq!(sent(&effects)[0]["id"], 7);
        assert_eq!(sent(&effects)[0]["result"], Value::Null);
    }

    #[test]
    fn an_unknown_request_is_answered_with_an_error_rather_than_dropped() {
        // A dropped request is indistinguishable from a hung client, and the server is blocked.
        let mut session = running();
        let effects = session.on_message(&json!({
            "jsonrpc": "2.0", "id": 9, "method": "window/showMessageRequest", "params": {},
        }));
        let reply = &sent(&effects)[0];
        assert_eq!(reply["id"], 9);
        assert_eq!(reply["error"]["code"], -32601);
    }

    #[test]
    fn an_unknown_notification_is_not_answered() {
        // The mirror rule: replying to a notification is a protocol error, and some servers close
        // the connection over it.
        let mut session = running();
        let effects = session.on_message(&json!({
            "jsonrpc": "2.0", "method": "telemetry/event", "params": {},
        }));
        assert!(sent(&effects).is_empty(), "{effects:?}");
    }

    #[test]
    fn progress_begin_scans_and_end_readies() {
        let mut session = running();
        assert_eq!(session.status(), SourceStatus::Ready);

        let effects = session.on_message(&json!({
            "jsonrpc": "2.0", "method": "$/progress",
            "params": { "token": "t", "value": { "kind": "begin", "title": "Indexing" } },
        }));
        let SourceStatus::Scanning { detail } = status(&effects).expect("status") else {
            panic!("{effects:?}");
        };
        assert_eq!(detail, "Indexing");

        let effects = session.on_message(&json!({
            "jsonrpc": "2.0", "method": "$/progress",
            "params": { "token": "t", "value": { "kind": "end" } },
        }));
        assert_eq!(status(&effects), Some(SourceStatus::Ready));
    }

    #[test]
    fn a_server_that_ends_indexing_with_no_diagnostics_is_ready_not_scanning() {
        // The mirror image of the panel's cardinal sin: claiming "we don't know" about a
        // workspace we do know is clean. A clean Rust workspace publishes nothing at all, so a
        // ready-requires-a-diagnostic rule would leave it Scanning for ever.
        let mut session = running();
        session.on_message(&json!({
            "jsonrpc": "2.0", "method": "$/progress",
            "params": { "token": "t", "value": { "kind": "begin", "title": "Indexing" } },
        }));
        let effects = session.on_message(&json!({
            "jsonrpc": "2.0", "method": "$/progress",
            "params": { "token": "t", "value": { "kind": "end" } },
        }));
        assert_eq!(status(&effects), Some(SourceStatus::Ready));
    }

    #[test]
    fn two_overlapping_progress_tokens_both_have_to_end() {
        // A counter would work here too — until a server ends a token twice, which is why this is
        // a set. rust-analyzer has done exactly that across versions.
        let mut session = running();
        for token in ["a", "b"] {
            session.on_message(&json!({
                "jsonrpc": "2.0", "method": "$/progress",
                "params": { "token": token, "value": { "kind": "begin", "title": "x" } },
            }));
        }
        session.on_message(&json!({
            "jsonrpc": "2.0", "method": "$/progress",
            "params": { "token": "a", "value": { "kind": "end" } },
        }));
        assert!(matches!(session.status(), SourceStatus::Scanning { .. }));

        // And ending one twice must not go negative and declare the server ready.
        session.on_message(&json!({
            "jsonrpc": "2.0", "method": "$/progress",
            "params": { "token": "a", "value": { "kind": "end" } },
        }));
        assert!(matches!(session.status(), SourceStatus::Scanning { .. }));

        session.on_message(&json!({
            "jsonrpc": "2.0", "method": "$/progress",
            "params": { "token": "b", "value": { "kind": "end" } },
        }));
        assert_eq!(session.status(), SourceStatus::Ready);
    }

    #[test]
    fn an_integer_progress_token_is_handled_like_a_string_one() {
        // The spec allows `string | integer`, and gopls uses integers.
        let mut session = running();
        session.on_message(&json!({
            "jsonrpc": "2.0", "method": "$/progress",
            "params": { "token": 3, "value": { "kind": "begin", "title": "Loading" } },
        }));
        assert!(matches!(session.status(), SourceStatus::Scanning { .. }));
        session.on_message(&json!({
            "jsonrpc": "2.0", "method": "$/progress",
            "params": { "token": 3, "value": { "kind": "end" } },
        }));
        assert_eq!(session.status(), SourceStatus::Ready);
    }

    #[test]
    fn a_publish_becomes_an_effect_with_an_absolute_path() {
        let mut session = running();
        let effects = session.on_message(&json!({
            "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
            "params": {
                "uri": "file:///repo/src/lib.rs",
                "diagnostics": [{
                    "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
                    "message": "boom",
                }],
            },
        }));
        let Some(Effect::Publish { abs_path, items }) = effects
            .iter()
            .find(|e| matches!(e, Effect::Publish { .. }))
            .cloned()
        else {
            panic!("{effects:?}");
        };
        assert_eq!(abs_path, "/repo/src/lib.rs");
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn an_empty_publish_is_still_a_publish() {
        // The whole point of `DiagnosticStore::publish` inserting an empty vec. If the session
        // swallowed empty lists, a file whose last error was just fixed would keep showing it.
        let mut session = running();
        let effects = session.on_message(&json!({
            "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
            "params": { "uri": "file:///repo/a.rs", "diagnostics": [] },
        }));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Publish { items, .. } if items.is_empty()))
        );
    }

    #[test]
    fn a_publish_for_a_uri_we_cannot_resolve_is_dropped_rather_than_panicking() {
        let mut session = running();
        let effects = session.on_message(&json!({
            "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
            "params": { "uri": "untitled:Untitled-1", "diagnostics": [] },
        }));
        assert!(!effects.iter().any(|e| matches!(e, Effect::Publish { .. })));
    }

    #[test]
    fn a_redundant_status_is_not_re_emitted() {
        // Every status reaches a coalescer and then the webview. Emitting `Ready` on each of a
        // hundred publishes would be a hundred snapshots of identical content.
        let mut session = running();
        let effects = session.on_message(&json!({
            "jsonrpc": "2.0", "method": "$/progress",
            "params": { "token": "t", "value": { "kind": "report" } },
        }));
        let first = status(&effects);
        let effects = session.on_message(&json!({
            "jsonrpc": "2.0", "method": "$/progress",
            "params": { "token": "t", "value": { "kind": "report" } },
        }));
        assert!(first.is_some());
        assert_eq!(status(&effects), None, "the same status was sent twice");
    }

    #[test]
    fn the_shutdown_ladder_sends_shutdown_then_exit_before_any_signal() {
        // gopls writes its cache on `exit`. A `SIGTERM` first costs the user that cache and makes
        // the next start re-index from nothing.
        let mut session = running();
        let effects = session.shutdown();
        let sent = sent(&effects);
        assert_eq!(sent[0]["method"], "shutdown");
        assert!(sent[0].get("id").is_some(), "shutdown is a request");
        assert_eq!(sent[1]["method"], "exit");
        assert!(sent[1].get("id").is_none(), "exit is a notification");
    }

    #[test]
    fn document_sync_messages_carry_the_shapes_a_server_expects() {
        let session = running();
        let Effect::Send(open) =
            session.did_open("file:///a.rs".into(), "rust", 1, "fn a(){}".into())
        else {
            panic!()
        };
        assert_eq!(open["params"]["textDocument"]["languageId"], "rust");

        let Effect::Send(change) = session.did_change("file:///a.rs".into(), 2, "fn b(){}".into())
        else {
            panic!()
        };
        // Full-text sync: one change with only `text` and no `range`.
        assert_eq!(change["params"]["contentChanges"][0]["text"], "fn b(){}");
        assert!(change["params"]["contentChanges"][0].get("range").is_none());
        assert_eq!(change["params"]["textDocument"]["version"], 2);
    }
}
