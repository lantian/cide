//! One IDE MCP server per project, and the bridge from its events into the workspace.
//!
//! # Why the app owns this and `cide-ide-mcp` does not
//!
//! The server crate knows about sockets, tools and a broker. It deliberately knows nothing
//! about tabs, panes or projects — so something has to translate "a `claude` somewhere asked
//! to show a diff" into "open a diff tab in project P", and that translation is the only
//! part that needs both vocabularies. It lives here.
//!
//! # The invariant that shapes every function below
//!
//! `openDiff` blocks an agent turn. The CLI sends it and waits; nothing else happens in that
//! conversation until an answer comes back. [`cide_ide_mcp::DiffBroker`] guarantees that
//! every way the *user* can disappear resolves the request — but it cannot know about the
//! ways the *app* can fail to show it in the first place. A project that closed between the
//! request arriving and the tab opening, a workspace mutation that is rejected by its own
//! validator, a pane id that collides: each of those is a path where the diff tab never
//! appears, and if any of them simply returns, the `claude` that asked waits for ever with
//! nothing on screen to explain it.
//!
//! So every early return in [`pump`] cancels first. The rule is that the request is
//! answered on every path out, and rejection is the safe answer because it leaves the file
//! alone.
//!
//! # Why a dedicated runtime
//!
//! The server is async; the command handlers and the signal-driven shutdown are not. Rather
//! than make the whole app async for one subsystem, this module owns a runtime and blocks on
//! it at the few points that need to. Shutdown blocks on the main or signal thread, which is
//! acceptable for the same reason [`crate::lifecycle::stop_children`] is: the process is on
//! its way out and the alternative is leaving a `claude` mid-turn.

use std::collections::HashSet;
use std::path::PathBuf;

use cide_ide_mcp::{
    CancelReason, DiffBroker, DiffOutcome, DiffRequest, IdeServer, ServerEvent, sweep_stale,
};
use cide_ipc::{
    DiffOrigin, DiffSpec, Pane, PaneId, PaneKind, PaneRole, ProjectId, TabKind, Workspace,
};
use dashmap::DashMap;
use tauri::{AppHandle, Manager};
use tokio::runtime::Runtime;

use crate::workspace_state::WorkspaceState;

/// A running server, and the handles the rest of the app needs from it.
struct Entry {
    server: IdeServer,
    port: u16,
    broker: DiffBroker,
}

/// Every project's IDE server.
pub struct IdeServers {
    rt: Runtime,
    servers: DashMap<ProjectId, Entry>,
    /// Bindings this app derived by ancestry, keyed by the pid they were derived **from**.
    ///
    /// `pane_of_pid` inside a server has one key per attributed process, and until M29 every
    /// one of them was a pid cide had forked itself — so `lifecycle::report_exit` could undo a
    /// binding with nothing but the pid it had just reaped. [`resolve_unbound_connections`]
    /// adds keys cide never forked: the `claude` a wrapper spawned, whose pid the reaper never
    /// sees. This is how those get undone with the child they came from, and it is the whole
    /// reason the resolver records the ancestor as well as the pane.
    ///
    /// Leaving them would not resurrect a dead pane on its own — the join needs an open
    /// connection and that connection died with the process — but a recycled pid would make an
    /// unrelated child answer for a pane, which is the hazard `IdeServer::unbind_pane`'s own
    /// note already spells out. One `Vec` because a pty child can spawn a CLI, have it exit,
    /// and spawn another before anything is reaped.
    derived: DashMap<u32, Vec<u32>>,
}

/// An [`IdeServers`] that has not yet been shown the restored workspace.
///
/// # Why this type exists
///
/// Two independent reviewers found the same hole and neither could close it: deleting the
/// `ensure_all` call from `lib.rs`'s `setup` left the entire suite green, with no dead-code
/// warning either. That is precisely the regression the call was added to fix — `ensure` had
/// been reachable only from `project_open`, so every launch that *restored* a workspace rather
/// than opening one gave its projects no server, no lockfile and no `CLAUDE_CODE_SSE_PORT`:
/// the headline feature absent on every launch after the first, silently, because a project
/// with no server is indistinguishable from Claude simply having no IDE.
///
/// The obvious guard is a test, and it is not available. Reaching the `setup` closure needs a
/// running Tauri app; `tauri`'s mock runtime is a different `Runtime` type, so using it would
/// mean making every `AppHandle` in this file generic over `R` — a large refactor of a crate
/// that is thin glue by policy, to guard one line.
///
/// So the two steps are welded together instead. [`new`](Self::new) hands back one of these,
/// and the only thing you can do with it is [`install`](Self::install), which ensures every
/// project's server **and** registers the state, in that order, in one call nobody can half-do.
///
/// What that does and does not buy, stated honestly, because the first attempt at this claimed
/// more than it delivered:
///
/// * The demonstrated regression — servers registered but never ensured — is now inexpressible.
///   It was two statements that had to agree, one before the app existed and one inside
///   `setup`; drift between them was the bug, and there is no longer a gap to drift in.
/// * Deleting the `install` call outright is a **compile error**, which is the case the
///   reviewers actually demonstrated. Nothing makes absent code a type error directly, so it
///   is caught one step downstream: `run` builds this and binds it, so removing its only use
///   leaves an unused binding, and `run` carries a scoped `deny(unused_variables)` for exactly
///   this reason. Verified by deleting the block and watching the build fail.
///
/// A first attempt returned `IdeServers` for the caller to `manage`, on the theory that only
/// this conversion could produce the managed type. That was wrong, and worth recording:
/// `Manager::manage` is generic over any `Send + Sync + 'static`, so `app.manage(pending)`
/// compiled happily, registered the wrong type, and left every `try_state::<IdeServers>()`
/// answering `None` — the identical silent failure, reached by a different route. Checked by
/// mutation, not by reading.
///
/// Ordering comes out of the same construction. The workspace has to be read before the state
/// can be registered at all, which puts every server up before `restore_windows` creates the
/// first webview — and therefore before any pane can spawn a child that reads
/// `CLAUDE_CODE_SSE_PORT` out of its environment at exec.
#[must_use = "an IDE subsystem that is never installed serves no project"]
pub struct PendingIdeServers(IdeServers);

impl PendingIdeServers {
    /// Build the runtime and clear our own abandoned lockfiles.
    ///
    /// The sweep runs once, at boot, before any server publishes. A lockfile left by a
    /// previous run that was killed points a `claude` at a port nothing is listening on,
    /// which the CLI reports as a broken IDE rather than as no IDE — worse than never having
    /// advertised. [`sweep_stale`] only ever removes files whose `ideName` is ours, so a
    /// user's VS Code or JetBrains lockfile in the same directory is never at risk.
    ///
    /// On this type rather than on [`IdeServers`] because that is what it returns: an
    /// `IdeServers` exists only on the far side of [`install`](Self::install).
    pub fn new() -> std::io::Result<Self> {
        let swept = sweep_stale();
        if swept > 0 {
            tracing::info!(swept, "removed stale cide lockfiles from a previous run");
        }

        Ok(Self(IdeServers {
            rt: tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("cide-ide")
                .build()?,
            servers: DashMap::new(),
            derived: DashMap::new(),
        }))
    }

    /// Start a server for every project the restored workspace holds, then manage the state.
    ///
    /// Failures are swallowed by [`IdeServers::ensure`] one project at a time, which is what
    /// we want here too: one project that cannot bind a port must not cost the others theirs.
    pub fn install(self, app: &AppHandle, ws: &Workspace) {
        for (project, roots) in servable_projects(ws) {
            self.0.ensure(app, project, roots);
        }
        app.manage(self.0);
    }
}

/// Point one IDE server's `getDiagnostics` at a project's store, if that project has one.
///
/// `McpDiagnostics` holds the *store*, not an `AppHandle`: `cide-ide-mcp` must not depend on
/// tauri, and this is what keeps that true while still answering from live data.
fn link_into(server: &cide_ide_mcp::IdeServer, app: &AppHandle, project: ProjectId) {
    if let Some(registry) = app.try_state::<crate::lsp::DiagnosticsRegistry>()
        && let Some(diagnostics) = registry.get(project)
    {
        server.set_diagnostics(std::sync::Arc::new(crate::lsp::McpDiagnostics::new(
            diagnostics.store(),
        )));
    }
}

/// Wire this project's diagnostics into its IDE server, from the diagnostics side.
///
/// # Why both ends and not an ordering
///
/// The two halves start independently — `IdeServers::ensure` and `DiagnosticsRegistry::ensure` —
/// and whichever runs second is the one that can complete the link. This used to be done only
/// from the IDE side, guarded by "the registry may not have this project yet", on the reasoning
/// that the order was a race that degraded to honesty.
///
/// It was not a race. `project_open` calls the IDE `ensure` **first**, every time, so the guard
/// was always false and `set_diagnostics` was never called from any real path: `getDiagnostics`
/// answered `[]` for every project, for ever, while looking exactly like a workspace with no
/// problems — and that is the one failure this whole surface exists to prevent. Calling from both
/// ends makes the order genuinely not matter, which is what the original comment claimed.
///
/// `set_diagnostics` overwrites, so calling this twice for one project is harmless.
pub fn link_diagnostics(app: &AppHandle, project: ProjectId) {
    if let Some(servers) = app.try_state::<IdeServers>()
        && let Some(entry) = servers.servers.get(&project)
    {
        link_into(&entry.server, app, project);
    }
}

impl IdeServers {
    /// Start a server for a project, or return the port of the one already running.
    ///
    /// Failure is logged and returns `None` rather than propagating: not being able to bind a
    /// loopback port costs IDE integration, and the terminal must keep working without it.
    /// That is the same rule the protocol module states — degrade the diff view, never break
    /// the terminal.
    pub fn ensure(&self, app: &AppHandle, project: ProjectId, roots: Vec<PathBuf>) -> Option<u16> {
        if let Some(entry) = self.servers.get(&project) {
            return Some(entry.port);
        }

        let server = match self.rt.block_on(IdeServer::start(roots)) {
            Ok(s) => s,
            Err(error) => {
                tracing::error!(%error, %project, "could not start the IDE server; panes will run without IDE integration");
                return None;
            }
        };

        let port = server.port();
        let broker = server.broker();
        let events = server.events();

        // Taken once, here, because `events()` hands the receiver over: a second caller would
        // silently take the stream from this pump and diffs would stop reaching the UI.
        self.rt
            .spawn(pump(app.clone(), project, events, broker.clone()));

        // Give `getDiagnostics` something to read, if the other half is up. If it is not, the
        // other half calls [`link_diagnostics`] when it arrives — see there for why this is done
        // from both ends rather than by ordering the two `ensure`s.
        link_into(&server, app, project);

        self.servers.insert(
            project,
            Entry {
                server,
                port,
                broker,
            },
        );
        tracing::info!(%project, port, "IDE server listening");
        Some(port)
    }

    /// The port to put in a child's `CLAUDE_CODE_SSE_PORT`.
    pub fn port(&self, project: ProjectId) -> Option<u16> {
        self.servers.get(&project).map(|e| e.port)
    }

    /// The broker, for answering a diff the user just resolved.
    pub fn broker(&self, project: ProjectId) -> Option<DiffBroker> {
        self.servers.get(&project).map(|e| e.broker.clone())
    }

    /// Tell the server which pane a connected `claude` belongs to.
    pub fn bind_pane(&self, project: ProjectId, pid: u32, pane: PaneId) {
        if let Some(entry) = self.servers.get(&project) {
            entry.server.bind_pane(pid, pane.to_string());
        }
    }

    /// Forget a reaped child's pane binding, in whichever project holds it.
    ///
    /// Not `unbind_pane(project, pid)`, and the missing argument is the point: pids are
    /// unique across the machine, so the pid alone identifies the binding, and the caller —
    /// `lifecycle::report_exit`, on `cide-pty`'s reaper thread — has a `SessionId` and no
    /// project. Threading the project down to it would mean taking the workspace lock on a
    /// reaper thread to answer a question the pid already answers.
    ///
    /// Sweeping every server is the honest shape for that: at most a handful of servers, one
    /// map removal each, and a pid that is not in a server's map costs a hash lookup.
    pub fn unbind_pid(&self, pid: u32) {
        for entry in self.servers.iter() {
            entry.server.unbind_pane(pid);
        }
        // And every descendant that was attributed *through* this pid. See `derived`: those
        // keys are pids cide never forked, so this reaper call is the only notice they get.
        if let Some((_, descendants)) = self.derived.remove(&pid) {
            for descendant in descendants {
                for entry in self.servers.iter() {
                    entry.server.unbind_pane(descendant);
                }
            }
        }
    }

    /// Which of this project's panes an addressed notification can actually reach.
    ///
    /// `None` when the project has no server at all — the same distinction `at_mentioned`
    /// draws, and for the same reason: "cide could not start an IDE server" and "no `claude`
    /// is on it" are different sentences and only the caller can put either on screen.
    ///
    /// Pane ids as strings, because that is the currency `cide-ide-mcp` deals in; compare
    /// against `PaneId::to_string()` exactly as [`Self::bind_pane`] produces it.
    pub fn addressable_panes(&self, project: ProjectId) -> Option<Vec<String>> {
        self.servers
            .get(&project)
            .map(|entry| entry.server.addressable_panes())
    }

    /// Connected CLIs in this project that no pane claims.
    ///
    /// Input to [`resolve_unbound_connections`], which is the only caller. Empty for a project
    /// with no server, which is the same answer as a project whose every connection is
    /// attributed — correct here, because there is nothing to resolve either way.
    pub fn unbound_pids(&self, project: ProjectId) -> Vec<u32> {
        self.servers
            .get(&project)
            .map(|entry| entry.server.unbound_pids())
            .unwrap_or_default()
    }

    /// Attribute a connected CLI to the pane whose child — or descendant — it is.
    ///
    /// `via` is the pid the attribution came from: the process cide actually forked. When it
    /// equals `pid` this is an ordinary binding and nothing is recorded, because
    /// `lifecycle::report_exit` already unbinds that pid when it reaps the child. When it does
    /// not, `pid` is a process cide never forked and the pairing is remembered so that reaping
    /// `via` takes it down too. See [`Self::derived`].
    fn bind_resolved(&self, project: ProjectId, pid: u32, via: u32, pane: PaneId) {
        self.bind_pane(project, pid, pane);
        if via != pid {
            self.derived.entry(via).or_default().push(pid);
        }
    }

    /// Tell every connected `claude` in a project where the editor selection is.
    ///
    /// Returns how many CLIs it reached; `None` when the project has no server at all. The
    /// two are different sentences — "cide could not start an IDE server" and "no `claude`
    /// has connected to it" — and only the caller can put either one on screen.
    pub fn selection_changed(
        &self,
        project: ProjectId,
        payload: cide_ide_mcp::SelectionChanged,
    ) -> Option<usize> {
        self.servers
            .get(&project)
            .map(|entry| entry.server.selection_changed_all(payload))
    }

    /// Put a file reference into one pane's `claude` prompt.
    ///
    /// Addressed to a single pane, unlike `selection_changed`. A mention is something the
    /// user aimed at a conversation — it lands in that prompt as text they are about to send
    /// — so broadcasting it would type into every Claude in the project at once.
    ///
    /// `None` when the project has no server; otherwise whether the notification actually
    /// went onto a socket. See [`cide_ide_mcp::Delivery`] — an `@`-mention that reaches
    /// nobody looks exactly like a menu item wired to nothing, and this is what lets the
    /// caller say which it was.
    pub fn at_mentioned(
        &self,
        project: ProjectId,
        pane: PaneId,
        payload: cide_ide_mcp::AtMentioned,
    ) -> Option<cide_ide_mcp::Delivery> {
        self.servers
            .get(&project)
            .map(|entry| entry.server.at_mentioned(&pane.to_string(), payload))
    }

    /// Stop a project's server, resolving anything it still owes.
    pub fn stop(&self, project: ProjectId) {
        let Some((_, entry)) = self.servers.remove(&project) else {
            return;
        };
        // `shutdown` rejects pending diffs *before* closing sockets, so a blocked `claude`
        // receives a plain refusal over a live connection instead of a transport error.
        self.rt.block_on(entry.server.shutdown());
        tracing::info!(%project, "IDE server stopped");
    }

    /// Stop every server. The quit path.
    pub fn stop_all(&self) {
        let projects: Vec<ProjectId> = self.servers.iter().map(|e| *e.key()).collect();
        for project in projects {
            self.stop(project);
        }
    }
}

/// Every project that wants a server, with the roots its lockfile should advertise.
///
/// A separate function from [`PendingIdeServers::install`] because this is the half that can be
/// tested: `ensure` binds a loopback port and writes into the user's real `~/.claude/ide`,
/// which is shared with whatever editors they are actually running and is no place for a
/// test suite to leave files — the same reason `cide-ide-mcp` tests against `attach` rather
/// than `start`.
///
/// Every root, not just `roots[0]`: the CLI matches its cwd against the lockfile's
/// `workspaceFolders`, so a `claude` started in a project's second root would otherwise not
/// see this server as its own.
/// Public because the language servers restore from the same list, in `lib.rs`'s `setup`: both
/// features need "every project the restored workspace holds", and two walks of one tree would be
/// two chances for a project to get one half and not the other.
pub fn servable_projects(ws: &Workspace) -> Vec<(ProjectId, Vec<PathBuf>)> {
    // What each restored window is *showing* comes first, and header order fills in behind
    // it. This ordering exists only for the cap below, and it is what makes the cap safe: in
    // header order a workspace whose active project sits past index 32 would launch with 32
    // servers for projects that are not on screen and none for the one that is — silently,
    // since a missing server looks exactly like Claude having no IDE. Ordering by what is
    // visible means the cap can only ever cost a project the user cannot currently see.
    //
    // Only `active` and the detached windows' own project, not every id in a stacked
    // window's `projects` list: in `Stacked` mode that list is every project in the
    // workspace, so honouring it would restore header order and hand the cap back its bug.
    let mut order: Vec<ProjectId> = Vec::with_capacity(ws.projects.len());
    let mut seen: HashSet<ProjectId> = HashSet::with_capacity(ws.projects.len());
    let mut want = |id: ProjectId, order: &mut Vec<ProjectId>| {
        // A window naming a project the workspace does not hold fails validation, so this is
        // belt-and-braces — but `servable_projects` runs on a file we have just read from
        // disk, and a `roots` lookup that misses would otherwise publish an empty lockfile.
        if ws.projects.contains_key(&id) && seen.insert(id) {
            order.push(id);
        }
    };
    for role in ws.windows.values() {
        match role {
            cide_ipc::WindowRole::Shell { active, .. } => {
                if let Some(id) = active {
                    want(*id, &mut order);
                }
            }
            cide_ipc::WindowRole::DetachedPane { project, .. }
            | cide_ipc::WindowRole::DetachedTab { project, .. } => want(*project, &mut order),
        }
    }
    for id in ws.projects.keys() {
        want(*id, &mut order);
    }

    let mut targets: Vec<(ProjectId, Vec<PathBuf>)> = order
        .into_iter()
        .map(|id| {
            let roots = ws.projects[&id]
                .roots
                .iter()
                .map(|r| r.path.clone())
                .collect();
            (id, roots)
        })
        .collect();

    // The same reasoning as `restore_windows`' window cap, and the same incident behind it: a
    // workspace here once accumulated 242 projects, and a launch that faithfully obeys such a
    // file binds 242 loopback ports and drops 242 lockfiles into `~/.claude/ide` — a
    // directory shared with whatever editors the user actually runs, which the CLI reads in
    // full every time it looks for an IDE. Serving the first few is a degraded launch; that
    // is a broken `claude` for everything else on the machine.
    //
    // The projects past the cap are not stranded: `project_open` on an already-open path
    // activates it and calls `ensure`, so re-opening the folder gives it a server.
    const MAX_RESTORED_SERVERS: usize = 32;
    if targets.len() > MAX_RESTORED_SERVERS {
        tracing::error!(
            projects = targets.len(),
            cap = MAX_RESTORED_SERVERS,
            "too many restored projects to serve; the rest get an IDE server when re-opened"
        );
        targets.truncate(MAX_RESTORED_SERVERS);
    }
    targets
}

/// Translate one project's server events into workspace mutations.
///
/// Every arm that cannot complete its mutation cancels the request it was handling. See the
/// module docs: an unanswered `openDiff` is a hung agent turn with no visible cause.
async fn pump(
    app: AppHandle,
    project: ProjectId,
    mut events: tokio::sync::mpsc::Receiver<ServerEvent>,
    broker: DiffBroker,
) {
    while let Some(event) = events.recv().await {
        match event {
            ServerEvent::DiffRequested(request) => {
                open_diff_tab(&app, project, &broker, request);
            }
            ServerEvent::DiffWithdrawn { tab_name } => {
                // The CLI closed the diff itself — its `beforeExit` does this. The broker has
                // already resolved the request; this only takes the tab off the screen.
                close_diff_tab_by_name(&app, project, &tab_name);
            }
            ServerEvent::Connected {
                pid,
                client_version,
                ..
            } => {
                tracing::debug!(
                    %project,
                    pid,
                    version = client_version.as_deref().unwrap_or("unknown"),
                    "a claude connected to the IDE server"
                );
                // **The record is written here and nowhere else.**
                //
                // Reaching this arm means a real `claude` completed the entire chain — read
                // our lockfile, chose the WebSocket transport, presented the auth header and
                // the `mcp` subprotocol, accepted our `initialize` reply and announced itself.
                // Nothing short of that produces this event, which is exactly what makes it
                // worth recording: a version compared against a constant proves that a human
                // typed a number, and this proves the protocol still works on this machine.
                //
                // Every rule about *whether* to write is in `cide_core::handshake::record` —
                // this event fires once per Claude pane, in an app that may have four open,
                // and a debounce written inline here would be a rule no test could reach.
                record_handshake(client_version);
                // And work out *which pane* just connected, when the pid alone does not say.
                // Here rather than only at send time so that a diff opened by a wrapper's
                // `claude` is attributed to the right pane too — attribution is not only about
                // mentions — and so the log line above is followed by the one that names the
                // pane, which is the pair a reader of a bug report needs.
                resolve_unbound_connections(&app, project);
            }
            ServerEvent::Disconnected { .. } => {}
            ServerEvent::OpenFile {
                path,
                start_line,
                end_line,
            } => {
                // Line numbers arrive already converted to 1-based by `tools::open_file`.
                tracing::debug!(%project, ?path, start_line, end_line, "openFile requested");
            }
        }
    }
}

/// Store what the CLI that just connected called itself, if it is news.
///
/// # Why this is a function and not three lines in the arm above
///
/// Two reasons, and the second is the one that matters. The first is that it does file I/O
/// inside an async task: `record` reads, decides and possibly writes, which is a handful of
/// microseconds on a state directory and is why it is called at most once an hour rather than
/// once per pane — but it is still I/O and it deserves a name.
///
/// The second is that this is the exact place this project has repeatedly stopped. The
/// milestone before this one shipped three features whose backends were complete and whose
/// call sites did not exist; `Capabilities::ide_protocol` sits three lines from
/// `claude_version()` hardcoded `false` with the comment "Wired in M6" long after M6 shipped.
/// A `Connected` arm that logged a version and did nothing with it would have been another.
/// The write is the feature; the event is only where it starts.
fn record_handshake(client_version: Option<String>) {
    let record = cide_core::handshake::Handshake::now(
        client_version,
        // The range as *this build* understands it, stored beside the version so a record
        // written by an older build is legible as stale rather than mistaken for a claim
        // about this one.
        cide_claude::version::verified_range(),
    );
    let _ = cide_core::handshake::record(&cide_core::handshake::handshake_path(), record);
}

/// Attribute every connected `claude` in `project` that no pane claims yet.
///
/// # The bug
///
/// A pane and a connection are joined by pid equality: `pane_bind_session` records the pid of
/// the process cide forked into the pty, the CLI announces `ide_connected {pid: process.pid}`,
/// and `cide_ide_mcp` matches them. That holds only while the process cide forked *is* the
/// process that opened the socket, and a launcher breaks it: `claude` on a given machine may be
/// a shim — a version manager, an `npm` launcher, a `bbin agent claude`-style wrapper — that
/// **spawns** the real CLI instead of `exec`ing it. cide then holds the wrapper's pid and the
/// CLI announces its own, the join finds nothing, and *Send lines to Claude* fails with
/// `cmd::file::not_connected`'s sentence: one session connected, nothing saying which pane it
/// is. The same shape covers a `claude` typed into a cide **shell** pane, which
/// `cide_ide_mcp::server`'s notes have described as unaddressable since it was written.
///
/// It looked platform-specific and is not: it was reported from a Mac whose `claude` is a
/// wrapper, against a Linux box whose `claude` is a symlink to the executable — one `execve`,
/// one process, pids equal. Nothing in this path is `cfg`-gated.
///
/// # The fix, and its one rule
///
/// The announced process is a *descendant* of one cide forked, so the join becomes an ancestry
/// walk: [`cide_core::proc::owning_ancestor`] climbs from the announced pid and stops at the
/// **first** pid this project has a pane for. Nearest wins, which is what makes a `claude`
/// inside a shell pane belong to the shell pane rather than to whatever pane is above it.
///
/// # Why it is safe to do this at all
///
/// Attribution decides where an `@`-mention is typed and which pane a diff is credited to.
/// Both are wrong-but-recoverable if this misfires, and it is hard to misfire: parentage is
/// exact, the walk is capped, and a chain with no owned ancestor is left unattributed rather
/// than guessed at — somebody else's `claude`, connected to this project through the lockfile
/// from a Terminal window, has no ancestor cide forked and stays exactly as unaddressable as
/// it is today.
///
/// # Why it runs twice
///
/// Once on `Connected`, which is the ordinary path, and once from `cmd::file::claude_send_lines`
/// before it decides a send has nowhere to go. The second is not belt-and-braces: the pane
/// binding is a workspace mutation the *frontend* makes after a spawn resolves, so a connection
/// that beats it — a fast wrapper, a loaded machine — would find `pane_pids` empty, and a
/// feature that depends on the order of two independent round trips is a feature that works on
/// the machine it was written on. Resolving again at the point of use costs one lock and one
/// syscall per unattributed connection, on a gesture a human just made, and there is normally
/// nothing to do.
pub fn resolve_unbound_connections(app: &AppHandle, project: ProjectId) {
    let Some(servers) = app.try_state::<IdeServers>() else {
        return;
    };
    let unbound = servers.unbound_pids(project);
    if unbound.is_empty() {
        return;
    }

    let panes = pane_pids(app, project);
    for pid in unbound {
        match cide_core::proc::owning_ancestor(
            pid,
            |p| panes.get(&p).copied(),
            cide_core::proc::parent_of_pid,
        ) {
            Some((via, pane)) => {
                tracing::info!(
                    %project, %pane, pid, via,
                    "attributed a claude to a pane through its ancestry"
                );
                servers.bind_resolved(project, pid, via, pane);
            }
            None => {
                // The sentence a bug report needs, and the one that was missing: it names the
                // chain that was walked and the children this project has, so the two can be
                // compared without a debugger. `PARENT_LOOKUP_WORKS` is in it because on a
                // platform that cannot walk at all the chain is one element and the *reason*
                // it is one element is not otherwise visible.
                tracing::warn!(
                    %project,
                    pid,
                    ancestry = ?cide_core::proc::ancestry_of(pid),
                    pane_children = ?panes,
                    lookup_works = cide_core::proc::PARENT_LOOKUP_WORKS,
                    "a claude is connected to this project and no pane owns it"
                );
            }
        }
    }
}

/// The pid of every live child this project's panes hold, and which pane holds it.
///
/// The map the ancestry walk climbs towards. Built from the workspace (which pane holds which
/// session) and the session registry (which session has which child), because those are the two
/// halves and nothing holds both — the same pair `pane_bind_session` joins, read in the other
/// direction.
///
/// **Every pane kind, not only Claude ones.** A `claude` typed into a shell pane is genuinely
/// running in that pane and attributing it there is the true answer; that it is not a pane an
/// `@`-mention can be *routed* to is `cmd::file::mention_candidates`' decision and belongs
/// there, not in a function about parentage. Diff attribution wants the true answer too.
///
/// The lock is released before the registry is touched: this runs on the IDE runtime and on a
/// command worker, and holding the workspace across another map's lock is how two subsystems
/// learn to wait for each other.
fn pane_pids(app: &AppHandle, project: ProjectId) -> std::collections::HashMap<u32, PaneId> {
    let (Some(state), Some(registry)) = (
        app.try_state::<WorkspaceState>(),
        app.try_state::<crate::state::SessionRegistry>(),
    ) else {
        return std::collections::HashMap::new();
    };

    let sessions: Vec<(cide_ipc::SessionId, PaneId)> = state.with(|ws| {
        let Ok(p) = cide_core::workspace::project(ws, project) else {
            return Vec::new();
        };
        p.tabs
            .iter()
            .flat_map(|tab| tab.tree.panes.values())
            .chain(p.detached.values())
            .filter_map(|pane| pane.session.map(|s| (s, pane.id)))
            .collect()
    });

    sessions
        .into_iter()
        .filter_map(|(session, pane)| Some((registry.get(session)?.child_pid()?, pane)))
        .collect()
}

/// Open the diff tab for a blocked `openDiff`, or reject it.
fn open_diff_tab(app: &AppHandle, project: ProjectId, broker: &DiffBroker, request: DiffRequest) {
    let Some(state) = app.try_state::<WorkspaceState>() else {
        // The app is being torn down. Rejecting is the only honest answer available.
        broker.cancel(&request.id, CancelReason::Shutdown);
        return;
    };

    let new_path = PathBuf::from(&request.params.new_file_path);
    let title = new_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| request.params.tab_name.clone());

    let spec = DiffSpec {
        title: title.clone(),
        old_path: PathBuf::from(&request.params.old_file_path),
        new_path,
        origin: DiffOrigin::ClaudeMcp {
            request_id: request.id.clone(),
        },
    };

    let pane = Pane {
        id: PaneId::new(),
        kind: PaneKind::Diff,
        role: PaneRole::Auxiliary,
        session: None,
        conversation: None,
        conversation_since: None,
        continues: None,
        title,
        docker: None,
    };

    let opened = state.update(|ws| {
        // Never a preview tab: this one blocks an agent turn on a promise about one
        // particular file, and the preview slot is by definition the tab a stray click may
        // re-point. `retarget_diff` refuses it a second time on the way in.
        let kind = TabKind::Diff {
            spec,
            preview: false,
        };
        cide_core::workspace::open_tab(ws, project, kind, pane)?;
        Ok(())
    });

    if let Err(error) = opened {
        // The project closed while the request was in flight, or the mutation failed its own
        // validator. Either way no tab will appear, so the turn must not be left waiting.
        tracing::warn!(%error, %project, "could not open a diff tab; rejecting the request");
        broker.cancel(&request.id, CancelReason::ProjectClosed);
    }
}

/// Take a diff tab off screen after the CLI withdrew it.
fn close_diff_tab_by_name(app: &AppHandle, project: ProjectId, tab_name: &str) {
    let Some(state) = app.try_state::<WorkspaceState>() else {
        return;
    };

    // The tab is found by the request id the CLI is closing, not by title: two diffs can
    // share a filename, and closing the wrong one would strand the other's agent turn.
    let target: Option<cide_ipc::TabId> = state.with(|ws| {
        let p = cide_core::workspace::project(ws, project).ok()?;
        p.tabs.iter().find_map(|tab| match &tab.kind {
            TabKind::Diff { spec, .. } => match &spec.origin {
                DiffOrigin::ClaudeMcp { request_id } if request_id == tab_name => Some(tab.id),
                _ => None,
            },
            _ => None,
        })
    });

    if let Some(tab) = target
        && let Err(error) = state.update(|ws| {
            // `force`: the tab being withdrawn is a `Diff`, which has no dirty flag and
            // therefore no unsaved edits to lose — and the CLI has already stopped waiting
            // on it, so refusing would leave a tab on screen that answers nothing.
            cide_core::workspace::close_tab(ws, project, tab, true)?;
            Ok(())
        })
    {
        tracing::warn!(%error, "could not close a withdrawn diff tab");
    }
}

/// Answer a diff the user resolved, and cancel any that a closing tab abandons.
///
/// Called from the tab-close path as well as from the explicit answer command, because
/// closing a diff tab *is* an answer: the protocol's `TAB_CLOSED` means accepted-as-proposed,
/// but a tab going away for any other reason must reject rather than accept, or a window
/// closing would write a change nobody approved.
pub fn resolve_or_cancel(
    servers: &IdeServers,
    project: ProjectId,
    request_id: &str,
    outcome: Option<DiffOutcome>,
) {
    let Some(broker) = servers.broker(project) else {
        return;
    };
    match outcome {
        Some(outcome) => {
            broker.resolve(request_id, outcome);
        }
        None => {
            broker.cancel(request_id, CancelReason::PaneClosed);
        }
    }
}

/// Cancel every `openDiff` that a set of tabs was showing.
///
/// Used by the close paths, which know which tabs are about to disappear but not which of
/// them are blocking an agent.
pub fn cancel_for_tabs(
    servers: &IdeServers,
    project: ProjectId,
    request_ids: impl IntoIterator<Item = String>,
    reason: CancelReason,
) {
    let Some(broker) = servers.broker(project) else {
        return;
    };
    for id in request_ids {
        broker.cancel(&id, reason);
    }
}

/// The `ClaudeMcp` request id of one tab, if it is a diff blocking an agent.
pub fn request_id_for_tab(
    state: &WorkspaceState,
    project: ProjectId,
    tab: cide_ipc::TabId,
) -> Option<String> {
    state.with(|ws| {
        let p = cide_core::workspace::project(ws, project).ok()?;
        p.tabs
            .iter()
            .find(|t| t.id == tab)
            .and_then(|t| match &t.kind {
                TabKind::Diff { spec, .. } => match &spec.origin {
                    DiffOrigin::ClaudeMcp { request_id } => Some(request_id.clone()),
                    _ => None,
                },
                _ => None,
            })
    })
}

/// The `ClaudeMcp` request ids of every diff tab in a project.
///
/// Read before a destructive mutation so the ids survive the tabs.
pub fn pending_request_ids(state: &WorkspaceState, project: ProjectId) -> Vec<String> {
    state.with(|ws| {
        cide_core::workspace::project(ws, project)
            .map(|p| {
                p.tabs
                    .iter()
                    .filter_map(|tab| match &tab.kind {
                        TabKind::Diff { spec, .. } => match &spec.origin {
                            DiffOrigin::ClaudeMcp { request_id } => Some(request_id.clone()),
                            _ => None,
                        },
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every restored project appears in the list `install` walks.
    ///
    /// **What this covers and what the compiler covers.** These tests hold the half that can
    /// be wrong — which projects are chosen, in what order, under the cap. They cannot reach
    /// the call site: `ensure` binds a port and writes into the user's real `~/.claude/ide`,
    /// and `tauri`'s mock app is behind a feature this build does not enable, the same
    /// limitation `hooks::live_in` documents.
    ///
    /// That gap used to be open, and two reviewers demonstrated it by deleting the call with
    /// every test still green. It is now closed by construction rather than by coverage: see
    /// [`PendingIdeServers`], which welds ensuring to managing so they cannot disagree, and
    /// `crate::run`'s scoped `deny(unused_variables)`, which makes deleting the call a build
    /// error. Verified by mutation both ways.
    ///
    /// Servers used to start only from `project_open`, so a launch that loaded
    /// `workspace.json` gave every project panes with no `CLAUDE_CODE_SSE_PORT`. The
    /// assertion is on the *set* of projects rather than on a count: serving only the active
    /// one is the shape this bug would most plausibly grow back in.
    #[test]
    fn every_restored_project_is_served() {
        let mut ws = Workspace::default();
        let atlas =
            cide_core::workspace::open_project(&mut ws, vec!["/home/dev/atlas".into()], None)
                .expect("a rooted project opens");
        let beacon =
            cide_core::workspace::open_project(&mut ws, vec!["/home/dev/beacon".into()], None)
                .expect("a rooted project opens");

        let served: Vec<ProjectId> = servable_projects(&ws)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(served, vec![atlas, beacon]);
    }

    /// The lockfile advertises the folders the CLI matches its cwd against, so a project's
    /// second root has to be among them — a `claude` started in it would otherwise read this
    /// server's lockfile as belonging to some other workspace.
    #[test]
    fn a_multi_root_project_advertises_all_of_its_roots() {
        let mut ws = Workspace::default();
        let id = cide_core::workspace::open_project(&mut ws, vec!["/home/dev/atlas".into()], None)
            .expect("a rooted project opens");
        cide_core::workspace::add_root(&mut ws, id, "/home/dev/atlas-docs".into())
            .expect("a second root is accepted");

        let roots = servable_projects(&ws)
            .into_iter()
            .find(|(p, _)| *p == id)
            .map(|(_, roots)| roots)
            .expect("the project is served");
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/home/dev/atlas"),
                PathBuf::from("/home/dev/atlas-docs"),
            ]
        );
    }

    /// A first launch restores no projects, and asking for zero servers is not a failure.
    #[test]
    fn an_empty_workspace_asks_for_nothing() {
        assert!(servable_projects(&Workspace::default()).is_empty());
    }

    /// A pathological file is capped rather than obeyed — one lockfile per project lands in
    /// a directory every `claude` on the machine reads.
    #[test]
    fn a_workspace_full_of_projects_is_capped() {
        let mut ws = Workspace::default();
        for i in 0..40 {
            cide_core::workspace::open_project(
                &mut ws,
                vec![format!("/home/dev/p{i}").into()],
                None,
            )
            .expect("a rooted project opens");
        }

        let served = servable_projects(&ws);
        assert_eq!(served.len(), 32);
        // The ones that are served are the header's leftmost, not an arbitrary subset: the
        // map's insertion order is the tab order the user sees.
        assert_eq!(served[0].1, vec![PathBuf::from("/home/dev/p0")]);
    }

    /// The cap must never take the server away from the project that is on screen.
    ///
    /// Straight header order does exactly that: a workspace of 40 whose active project is
    /// the last one would launch having bound 32 ports for projects behind the tab strip and
    /// none for the one in front of the user — and a project with no server is
    /// indistinguishable, from the pane, from Claude simply having no IDE.
    #[test]
    fn the_cap_never_drops_the_project_a_window_is_showing() {
        let mut ws = Workspace::default();
        let ids: Vec<ProjectId> = (0..40)
            .map(|i| {
                cide_core::workspace::open_project(
                    &mut ws,
                    vec![format!("/home/dev/p{i}").into()],
                    None,
                )
                .expect("a rooted project opens")
            })
            .collect();
        // Through the real gesture: `rebuild_windows` leaves `active` on the first project
        // opened, and clicking the fortieth header tab is what moves it.
        let on_screen = ids[39];
        cide_core::workspace::activate_project(&mut ws, on_screen);

        let served = servable_projects(&ws);
        assert_eq!(served.len(), 32);
        assert_eq!(
            served[0].0, on_screen,
            "the visible project is served first"
        );
        // And the cap still spends the rest of its budget on the header's leftmost, rather
        // than on whatever the window list happened to mention.
        assert_eq!(served[1].1, vec![PathBuf::from("/home/dev/p0")]);
    }

    /// A detached window has no tab strip and no `active` — its project is what it shows.
    #[test]
    fn a_detached_window_counts_as_showing_its_project() {
        let mut ws = Workspace::default();
        let ids: Vec<ProjectId> = (0..40)
            .map(|i| {
                cide_core::workspace::open_project(
                    &mut ws,
                    vec![format!("/home/dev/p{i}").into()],
                    None,
                )
                .expect("a rooted project opens")
            })
            .collect();
        ws.windows.insert(
            cide_ipc::WindowLabel("pane:test".into()),
            cide_ipc::WindowRole::DetachedPane {
                project: ids[38],
                tab: cide_ipc::TabId::new(),
                pane: PaneId::new(),
            },
        );

        let served: Vec<ProjectId> = servable_projects(&ws).into_iter().map(|(p, _)| p).collect();
        assert!(served.contains(&ids[38]));
    }

    /// A derived binding is undone by reaping the child it was derived from.
    ///
    /// The rule [`IdeServers::derived`] exists for. `lifecycle::report_exit` unbinds the pid it
    /// reaped, and the CLI behind a wrapper has a pid that reaper never sees — so without the
    /// pairing, the one binding cide invented is the one binding nothing ever removes, and a
    /// recycled pid would answer for a dead pane.
    ///
    /// **What it does not cover.** There is no server in this map, so `bind_pane` and
    /// `unbind_pane` reach nothing: this asserts the bookkeeping and not the routing. Starting
    /// a real server needs a bound port and a lockfile in the user's `~/.claude/ide`, which is
    /// the same limitation the test above states. The routing half is
    /// `cide-ide-mcp`'s `a_connection_nothing_bound_is_reported_until_something_binds_it`,
    /// which drives an actual socket.
    #[test]
    fn reaping_a_wrapper_forgets_the_cli_it_spawned() {
        let servers = IdeServers {
            rt: tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a runtime"),
            servers: DashMap::new(),
            derived: DashMap::new(),
        };
        let project = ProjectId::new();
        let pane = PaneId::new();

        // 7100 is the wrapper cide forked; 7101 is the `claude` it spawned and the pid that
        // reached the socket.
        servers.bind_resolved(project, 7101, 7100, pane);
        assert_eq!(
            servers.derived.get(&7100).map(|v| v.clone()),
            Some(vec![7101]),
            "the invented binding is recorded against the child that explains it"
        );

        servers.unbind_pid(7100);
        assert!(
            servers.derived.get(&7100).is_none(),
            "and reaping that child takes it with it"
        );

        // The ordinary case records nothing: `report_exit` already unbinds a pid cide forked,
        // and a second copy of that fact is a second thing to keep in step.
        servers.bind_resolved(project, 7200, 7200, pane);
        assert!(servers.derived.is_empty());
    }
}
