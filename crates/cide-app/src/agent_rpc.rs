//! `$CIDE_AGENT_SOCK` — the socket a `claude` child's MCP bridge talks to. (M18)
//!
//! # Shape
//!
//! One listener per process, at `$XDG_RUNTIME_DIR/cide-agents-<pid>.sock`, `0600`, unlinked on
//! drop. Every child cide spawns is given the path in `CIDE_AGENT_SOCK` and an `--mcp-config`
//! naming `cide-hook mcp`; that bridge connects here and proxies its client's JSON-RPC verbatim.
//! The vocabulary it proxies is [`cide_agents::tools`] and lives nowhere else — the bridge holds
//! no schema at all, so a `cide-hook` left over from an older install cannot advertise a tool
//! this build has removed.
//!
//! # Why this is not the hook socket
//!
//! [`crate::hooks::HookServer`] is the closest thing in the process and the lifecycle below is
//! modelled on it line for line — accept thread, per-connection thread, stale-file removal,
//! `Drop` unlinking. Three differences, and each is a reason the two are separate sockets rather
//! than one:
//!
//! * **One long-lived connection per client, not one per message.** A hook invocation is a fresh
//!   short-lived process, so connection-per-frame is the only shape available to it. An MCP
//!   session is a *conversation*: request ids are only meaningful against the connection that
//!   issued them, and — the part that matters here — identity is bound from the header line
//!   below, once. Reconnecting per message would re-run that binding for every message or lose
//!   it entirely.
//! * **Requests need replies.** The hook socket's contract is write-and-forget: every failure is
//!   an exit 0 because it runs on the critical path of a turn and a hook that errors puts noise
//!   in someone's session. Here the caller is *blocked* on the answer. A request that goes
//!   unanswered is an agent's turn that never ends, which is the failure class
//!   `cide_ide_mcp`'s `openDiff` exists to guard against, one socket over.
//! * **Separate lifetimes, and separate serialisation.** `hooks.rs` funnels every frame through
//!   one `cide-hook-apply` thread on purpose, so a `PostToolUse` cannot land after a `Stop`. A
//!   tool call here does file I/O through `cide_tasks::TaskStore`, and queueing that behind the
//!   hook applier would make a slow disk stall the session state machine.
//!
//! # The header line is the authorisation
//!
//! Before any JSON-RPC, the bridge writes one line:
//!
//! ```json
//! {"hello":"cide-mcp","v":1,"run":null,"session":"<uuid>","pid":1234}
//! ```
//!
//! **Both ids come from the child's own environment**, never from a payload the caller composes
//! — the `spawned_as` rule `cide_hook`'s `forward` states and this socket depends on completely.
//! A caller that could name its own identity could sign a comment as the user, or, once the
//! orchestration tools land, dispatch as though it were the product owner.
//!
//! That is what makes the scoping **structural rather than prompted**. [`resolve`] asks three
//! questions in a fixed order — is this a run this process dispatched; failing that, is this
//! session some project's [`cide_ipc::Project::primary_session`]; failing that, is it a Claude
//! pane of some project at all? — and the answer decides the tool list for the whole connection:
//!
//! | the header names | served |
//! | --- | --- |
//! | a run this process dispatched | the **nine** `cide_task_*` tools, scoped to that run's project, signing every comment as that run's role |
//! | a Claude pane of a project — its console (`primary_session`) or any other | those nine **and** the eleven orchestration tools, scoped to **that** project, signing as [`TaskAuthor::Orchestrator`] |
//! | anything else, or nothing | a valid `initialize` and an **empty** tool list |
//!
//! Never a crash, and never another project's tasks. The primary-session question is still asked,
//! and first: not to decide the tool list any more, but to mark the console as such
//! ([`Scope::Pane::primary`]), which is what picks the opening sentence of the paragraph
//! `cmd::session` appends and nothing else.
//!
//! # Why the third row exists (M30)
//!
//! It did not, and the hole it left was silent in the worst way. M18 wrote this table when a
//! connection could only be one of two things: a run the harness spawned, or the pinned console.
//! M28 added a third — `TasksPanel/openSession.ts` opens a task's conversation as an ordinary
//! *second* Claude pane, and `spec_dispatch_to_session` types `cmd::agents::opening_prompt` into
//! it. That pane is not a run (the harness sets `CIDE_RUN`; the split machinery does not) and it
//! is not `primary_session`, so it fell to the last row: `initialize` succeeded, the CLI drew a
//! connected `cide` server, and `tools/list` came back empty. The model was then told in its
//! first sentence to *"read it with `mcp__cide__cide_task_get`"* with no such tool in its
//! context, and what it actually did — in a real project, and it is what sent this looking — was
//! hand-write `.cide/tasks.json` from a guessed schema and corrupt the file.
//!
//! **What the row did not widen is the boundary.** The session still has to be one this process
//! minted *and* still bound to a Claude pane, so a `claude` a user started by hand in a cide
//! shell — which inherits `CIDE_AGENT_SOCK` from that shell but is given no `CIDE_SESSION` — has
//! nothing to match with, exactly as before.
//!
//! # Why the two pane rows became one (M40)
//!
//! M30's row served a second Claude pane the task tools and withheld the orchestration
//! ones, on the argument that those belong to the console the product-owner paragraph was
//! appended to. The hole that left was the mirror of M30's: a second pane could already *start*
//! a run — assigning or @mentioning a role starts it, through `crate::task_triggers` — and never
//! hear about it, because the finish was announced to the console, which had not asked. Once the
//! announcement became something a dispatch chooses ([`cide_ipc::RunNotify`], read at the edge by
//! [`note_run_over`]), the withholding bought nothing: the boundary that matters is *run versus
//! pane*, and the run row is asked first and can never fall through to this one. So every Claude
//! pane of a project may dispatch, and the paragraph now reaches every one of them with an
//! opening sentence that says which it is. Agent-dispatches-agent stays closed by construction.
//!
//! **The author is [`TaskAuthor::Orchestrator`], not [`TaskAuthor::User`]**, and that is a
//! refusal rather than a label: `cide_tasks::TaskStore::edit` gates the comment edit and delete
//! arms on `author == User`, so signing a model's connection as the user would hand every
//! conversation pane the power to rewrite somebody else's comment — the one thing the
//! append-only rule exists to prevent. Orchestrator is the nearest honest fit for a session a
//! *person* is driving inside this project, which is the same call `crate::task_triggers` makes
//! and states at its own `consider`.
//!
//! **The run row is asked first, and that ordering is part of the boundary rather than a
//! micro-optimisation.** A subagent is spawned with both `CIDE_RUN` and `CIDE_SESSION`, and its
//! `CIDE_SESSION` is a session id this process minted — so a run whose session ever came to be a
//! project's `primary_session` (it is not today; it would take a `claude.mirror` plus a re-bind)
//! would, under the other order, be handed the dispatch tools. Asking the run question first
//! means a connection that is a run can never be read as anything else.
//!
//! That is the whole of the answer to *may an agent dispatch an agent*: the eleven orchestration
//! tools are never in a run's `tools/list`, and [`call_tool`] refuses a name the connection was
//! not shown, so a run that guesses `cide_agent_dispatch` is answered `METHOD_NOT_FOUND` before
//! anything reaches the registry. Closed by construction rather than by asking a model not to.
//!
//! # The scope is decided once, and that is a promise to the client
//!
//! `initialize` advertises `capabilities.tools.listChanged: false`, so a client is entitled to
//! call `tools/list` once and cache it for the session. A scope re-derived per message could
//! therefore differ from the list the model is holding — the project's `primary_session` moves
//! whenever the console pane binds a new conversation (`workspace::bind_session` rewrites it on
//! every restart), so a long-lived orchestrator could silently lose its tools mid-turn and be
//! told "no such tool" for something it can still see. Resolving once at connect makes the
//! advertised list true for as long as the connection lives.
//!
//! # Two things this must never do
//!
//! **Never block the accept loop on a tool call.** A tool call takes a `TaskStore`'s mutex and
//! writes a file; on the accept thread that would stop every *other* agent connecting for the
//! length of it. One thread per connection, as `hooks.rs` does — and here a thread is per live
//! agent rather than per hook invocation, so the count is bounded by how many agents are running.
//!
//! **Never hold a `DashMap` shard guard across disk I/O.** [`crate::tasks_state`] makes this
//! argument for its own flusher and it is the same one here: `TasksStores::get`/`ensure` clone
//! the `Arc<TaskStore>` out, and everything below works from the clone.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use cide_agents::Delivery;
use cide_agents::tools::{self, AgentSink, Integrated, Stopped, TaskSink, ToolResult};
use cide_ipc::{
    AgentDef, AgentId, AgentRoster, AgentRun, AttachTarget, ChangeName, DispatchRequest, Harness,
    LlmSettings, OrchestrationConfig, OrchestrationPatch, PaneKind, ProjectId, ProjectOverrides,
    RunId, RunNotify, RunState, SessionId, SessionState, Task, TaskAuthor, TaskEdit, TaskId,
    TaskNew, TaskRow,
    agents::{AgentDraft, AgentSaveOutcome, AgentScope},
};
use cide_ipc::{Project, Workspace};
use cide_tasks::TaskStore;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

use crate::agents::{AgentRegistry, DeathFacts, StopBy, StopHow};
use crate::state::SessionRegistry;
use crate::task_triggers::TaskMutation;
use crate::tasks_state::TasksStores;
use crate::workspace_state::WorkspaceState;

/// The header's tag and the schema version this build understands. Both are `cide_hook::mcp`'s;
/// changing either without changing the bridge takes every agent's tools away.
const HELLO: &str = "cide-mcp";
const HELLO_VERSION: u32 = 1;

/// How long a write to a bridge may block before the connection is written off.
///
/// **Symmetric with the bridge's own**, and both ends need one for the same reason: cide can
/// `SIGSTOP` a paused agent's whole process group, and the bridge is inside that group. Without a
/// timeout here, writing a reply into a frozen agent's socket blocks this connection's thread the
/// moment the kernel buffer fills — so pausing one agent would cost a thread until it was
/// resumed, and there is no bound on how long that is.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// Echoed back when a client sends `initialize` without a `protocolVersion`.
///
/// The same constant `cide-ide-mcp` and the bridge carry, used the same way: the reply echoes the
/// client's version and only falls back to this, because inventing a version the client did not
/// offer ends the handshake.
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;

/// The socket, and nothing else.
///
/// There is deliberately no per-connection registry here. Nothing in the app addresses a live
/// agent connection — the tools are all request/response — so a table of them would be state with
/// no reader, and a table of live sockets with no reader is a leak with a lock on it.
pub struct AgentRpcServer {
    path: PathBuf,
}

impl AgentRpcServer {
    /// Bind the socket and start accepting.
    ///
    /// Failure costs the task tools — a child's bridge finds nothing on `CIDE_AGENT_SOCK` and
    /// serves an empty tool list, which is a state `cide_hook::mcp` is explicitly written to
    /// degrade into — but it costs nothing else, so this reports rather than refusing to launch.
    pub fn start(app: AppHandle) -> std::io::Result<Self> {
        let path = socket_path();

        // A socket left by a previous run of this same pid would make `bind` fail with
        // `AddrInUse`. Removing it is safe: the path carries our pid, so nothing else owns it.
        let _ = std::fs::remove_file(&path);

        let listener = UnixListener::bind(&path)?;
        // The socket is a control channel into this process, and one that can write the user's
        // repository. `XDG_RUNTIME_DIR` is usually 0700 but `temp_dir()` is not, so the socket is
        // narrowed rather than trusted to inherit — `HookServer::start`'s reasoning, and it
        // matters more here, because this end mutates a committed file.
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }

        // Built once, outside the loop: it holds the `AppHandle` every connection resolves its
        // scope against, and cloning an `Arc` per connection is cheaper than cloning the handle.
        let bind = app_binder(app);

        thread::Builder::new()
            .name("cide-agent-accept".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let bind = Arc::clone(&bind);
                    // One thread per connection, and it lives as long as the agent does. The
                    // accept loop must never be the thing that runs a tool call: that call takes
                    // a store's mutex and writes a file, and doing it here would stop every other
                    // agent connecting for the length of it. See the module header.
                    let _ = thread::Builder::new()
                        .name("cide-agent-conn".into())
                        .spawn(move || serve(stream, bind.as_ref()));
                }
            })?;

        tracing::info!(path = %path.display(), "agent rpc socket listening");
        Ok(Self { path })
    }

    /// The value for a child's `CIDE_AGENT_SOCK`.
    pub fn socket(&self) -> &Path {
        &self.path
    }
}

impl Drop for AgentRpcServer {
    fn drop(&mut self) {
        // A socket file outliving its process is a path children would connect to and then wait
        // on with nothing on the other end — and waiting is exactly what this socket's callers do.
        let _ = std::fs::remove_file(&self.path);
    }
}

fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    dir.join(format!("cide-agents-{}.sock", std::process::id()))
}

// --- one connection ----------------------------------------------------------------------------

/// The header line, as the bridge writes it.
#[derive(Debug, Clone, Deserialize)]
struct Hello {
    hello: String,
    v: u32,
    /// The `CIDE_RUN` of a dispatched subagent — `cide_agents::harness::claude` sets it on
    /// every run's child, and nothing else in the process sets it. Resolved **before** the
    /// session below; see the module header on why that order matters.
    #[serde(default)]
    run: Option<String>,
    /// The `CIDE_SESSION` cide spawned this child under. For a Claude pane this *is* the id the
    /// pane is filed under, which is what makes the primary-session comparison below meaningful.
    #[serde(default)]
    session: Option<String>,
    #[serde(default)]
    pid: Option<u32>,
}

/// What one connection may reach.
///
/// No longer `Copy`, and the two payloads that cost it are the point: a run's connection has to
/// carry *which role it is*, because that is what signs its comments, and identity read out of
/// the connection is the whole `spawned_as` rule this module rests on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Scope {
    /// The header named a Claude pane of a project: its console (`primary` — the pane the
    /// product-owner paragraph opens for as such) or any other Claude pane, a task's
    /// conversation or a second console the user split off. Every tool, scoped to that project,
    /// signing as [`TaskAuthor::Orchestrator`]. (M30 for the row; M40 for the tools.)
    ///
    /// `session` is the pane's own id out of the header, carried so a dispatch's `notify: here`
    /// and an assignment's default report land in *this* pane — see [`ProjectTools::session`].
    /// The author is fixed at the binder and is never `User` — see the module header.
    Pane {
        project: ProjectId,
        session: SessionId,
        primary: bool,
    },
    /// The header named a run this process dispatched. The task tools, and nothing else.
    Run {
        project: ProjectId,
        agent: AgentId,
        /// The role's display label, copied at dispatch — see `crate::agents::LiveRun`. It is
        /// what `TaskAuthor::Agent` shows a person reading the comment log, and it must still
        /// read correctly after the role has been renamed or deleted.
        label: String,
        /// The run's working directory — its worktree — which is what a relative path in
        /// `cide_task_attach` is read against. From the registry, never from the child. (M39)
        cwd: PathBuf,
    },
    /// A run started from the GitLab MR panel to review one merge request. (M85) Served the
    /// `cide_mr_*` tools and **nothing** of the tracker or the orchestration — see
    /// `cide_agents::review`'s header for why the line is drawn there.
    Review {
        run: RunId,
        review: String,
        label: String,
        harness: Harness,
        cwd: PathBuf,
    },
    /// The header named nothing this process can place. A valid server with no tools.
    Unscoped,
}

/// The tools one connection is allowed to call.
///
/// A trait so the JSON-RPC layer below can be driven from a test with no `AppHandle` — `tauri`'s
/// mock app is behind a feature this build does not enable, so anything that takes one is
/// untestable here. This is the same split `hooks.rs` makes for `live_in`/`forget_in` and for the
/// same reason, and it is why [`respond`] has tests at all.
pub(crate) trait ToolAccess: Send + Sync {
    fn names(&self) -> &[&'static str];
    fn call(&self, name: &str, arguments: &Value) -> ToolResult;
}

/// An unscoped connection: a real server, no tools.
struct NoTools;

impl ToolAccess for NoTools {
    fn names(&self) -> &[&'static str] {
        &[]
    }

    fn call(&self, name: &str, _arguments: &Value) -> ToolResult {
        // Unreachable through [`respond`], which checks `names` first. Answered rather than
        // unwrapped because a panic in a connection thread aborts the process under
        // `panic = "abort"`.
        ToolResult::error(format!("{name} is not available on this connection"))
    }
}

/// A connection bound to one project's tracker.
struct ProjectTools {
    app: AppHandle,
    project: ProjectId,
    /// Who a comment written over this connection is from. Decided from the header, never from
    /// the payload — see the module header.
    author: TaskAuthor,
    /// Whether this connection is a Claude *pane* — and therefore whether the eleven orchestration
    /// tools exist for it at all. False for a run: the line this module exists to draw.
    ///
    /// A `bool` and not a second `ToolAccess` implementation, because the two differ in exactly
    /// two places — the advertised names and one `or_else` below — and a parallel type would
    /// have to re-state the store lookup, the author and the broadcast, which is where a copy
    /// would eventually diverge.
    orchestrator: bool,
    /// The pane's own session, for a pane connection; `None` for a run. (M40) It is what
    /// `notify: here` resolves to on a dispatch, and what a run started by an assignment made
    /// over this connection reports back to — so the pane that asked is the pane that hears.
    session: Option<SessionId>,
    /// Where a relative path in an argument is read from, when the connection is a run's:
    /// its worktree. `None` for a console pane, whose cwd is the project root — filled in as
    /// exactly that when the sink is built, once the store (and so the root) is known. (M39)
    cwd: Option<PathBuf>,
}

impl ToolAccess for ProjectTools {
    fn names(&self) -> &[&'static str] {
        if self.orchestrator {
            tools::tool::EVERY
        } else {
            tools::tool::ALL
        }
    }

    fn call(&self, name: &str, arguments: &Value) -> ToolResult {
        let Some(store) = self.store() else {
            // The project closed while this agent was mid-turn. A sentence rather than a
            // JSON-RPC error, so the model reads it and stops rather than reporting a broken
            // server it will retry.
            return ToolResult::error(
                "this project is no longer open in cide, so its task tracker cannot be reached",
            );
        };

        let sink = StoreSink {
            store: Arc::clone(&store),
            project: self.project,
            author: self.author.clone(),
            changed: AtomicBool::new(false),
            mutations: Mutex::new(Vec::new()),
            cwd: self
                .cwd
                .clone()
                .unwrap_or_else(|| store.root().to_path_buf()),
        };
        // The task family first, then — **only for a pane** — the orchestration one. The
        // [`RegistrySink`] is built *inside* the arm, so a run's connection never constructs one
        // at all: "which vocabulary a caller can reach" stays a fact about which value exists on
        // this stack frame. `cide_agent_dispatch` arriving from a run therefore falls through to
        // the refusal below rather than to a registry — belt-and-braces, since [`call_tool`]
        // already refused the name against `names()`. The `?` on the session is the same fact
        // from the other side: a pane always has one, a run's `orchestrator` is false.
        let result = tools::dispatch(name, arguments, &sink)
            .or_else(|| {
                if !self.orchestrator {
                    return None;
                }
                let agents = RegistrySink {
                    app: self.app.clone(),
                    project: self.project,
                    session: self.session?,
                };
                tools::dispatch_orchestration(name, arguments, &agents)
            })
            .unwrap_or_else(|| ToolResult::error(format!("no such tool: {name}")));

        // Broadcast **after** the mutation and outside any lock the sink held. Without this an
        // agent's task lands in the file and in memory and nothing on screen moves: the panel
        // would only catch up on the next unrelated `tasks_board`, which is the class of failure
        // that looks like a dead UI. `broadcast` re-reads the store and emits
        // `cide://tasks-changed`, whose `rev` lets a window drop an out-of-order snapshot.
        if sink.changed.load(Ordering::Relaxed) {
            crate::tasks_state::broadcast(&self.app, self.project, &store);
        }
        // And after the broadcast, the assignment-starts-work question — for every author, with
        // the refusal in `autodispatch::trigger`'s author gate; see `StoreSink::mutations`. A
        // run such an edit starts reports back to *this* pane (M40): the pane that assigned is
        // the one waiting. A run's connection has no session and cannot start one anyway.
        let notify = self
            .session
            .map_or(RunNotify::Primary, |session| RunNotify::Session { session });
        let mutations = sink.mutations.into_inner();
        // A run that just set its task to review has, by its own account, finished: verify its
        // branch now, so the reviewer's merge finds the answer ready instead of waiting on it.
        // (M83) Warming only — the merge is where the result is enforced.
        for mutation in &mutations {
            if let TaskAuthor::Agent { agent, .. } = &mutation.author
                && mutation.after.status == cide_ipc::TaskStatus::Review
                && mutation.before.as_ref().map(|t| t.status) != Some(cide_ipc::TaskStatus::Review)
            {
                crate::milestones::prewarm(
                    &self.app,
                    self.project,
                    agent.clone(),
                    mutation.after.id.clone(),
                );
            }
        }
        crate::task_triggers::consider(&self.app, self.project, mutations, notify);
        result
    }
}

impl ProjectTools {
    /// This project's tracker, opening it if this is the first ask.
    ///
    /// The `Arc` is cloned out of the `DashMap` before anything touches the disk — see the module
    /// header. `ensure` is idempotent and is called here as a fourth site for the reason
    /// `TasksStores::ensure` already documents: a registry reachable only from `project_open` is
    /// one a restoring launch comes up without.
    fn store(&self) -> Option<Arc<TaskStore>> {
        let stores = self.app.try_state::<Arc<TasksStores>>()?;
        if let Some(open) = stores.get(self.project) {
            return Some(open);
        }
        let workspace = self.app.try_state::<WorkspaceState>()?;
        let root = crate::tasks_state::project_root(&workspace, self.project).ok()?;
        Some(stores.ensure(self.project, &root))
    }
}

/// [`TaskSink`] over the one store that owns this project's `.cide/tasks.json`.
///
/// Every mutation in the process funnels through `TaskStore`'s single mutex — the panel's and
/// every agent's alike — which is the whole reason a single JSON file is safe here. This type is
/// the agent half of that funnel and does nothing but adapt shapes.
///
/// **It holds no `AppHandle`, and that is worth keeping.** A store plus an author is everything
/// the nine task tools need, so this type is constructible in a test — which is what
/// `a_store_backed_sink_mutates_and_reports_that_it_did` does, and `tauri`'s mock app is behind a
/// feature this build does not enable. The orchestration half needs managed state and therefore
/// lives in [`RegistrySink`] beside it rather than widening this.
struct StoreSink {
    store: Arc<TaskStore>,
    project: ProjectId,
    author: TaskAuthor,
    /// Set by any call that changed the file, so the connection broadcasts once and only when
    /// something moved. A `tasks-changed` per `cide_task_list` would be an event to every window
    /// for a read.
    changed: AtomicBool,
    /// What the call mutated, as plain data for [`crate::task_triggers::consider`] — drained by
    /// [`ProjectTools::call`] after the broadcast. Recorded for *every* author, run-authored
    /// mutations included: the refusal that keeps a subagent from spawning a subagent is
    /// `autodispatch::trigger`'s author gate, applied centrally where its test sees it, not a
    /// filter here that somebody could later remove. Pure data, so this type stays
    /// `AppHandle`-free and constructible in the sink test above.
    mutations: Mutex<Vec<TaskMutation>>,
    /// See [`ProjectTools::cwd`]; resolved to a value by the time a sink exists. (M39)
    cwd: PathBuf,
}

impl TaskSink for StoreSink {
    fn list(&self) -> Result<Vec<TaskRow>, String> {
        Ok(self.store.list())
    }

    fn get(&self, id: &TaskId) -> Result<Option<Task>, String> {
        Ok(self.store.get(id))
    }

    fn create(
        &self,
        title: &str,
        body: &str,
        agent: Option<&AgentId>,
        change: Option<&ChangeName>,
        links: &[cide_ipc::TaskLinkSpec],
        attachments: &[PathBuf],
        inbox: bool,
    ) -> Result<Task, String> {
        let req = TaskNew {
            // Carried because the wire shape has it; the store ignores it and knows its own
            // project from the path it was opened with. Both facts are in `TaskStore::create`.
            project: self.project,
            title: title.to_string(),
            body: Some(body.to_string()),
            agent: agent.cloned(),
            // Always. `TaskSink::create` has no status parameter and the `cide_task_create` tool
            // advertises none, so a run cannot mint a task that is already `Done` — the
            // restriction `TaskNew::status` describes, in the one line that enforces it. The
            // one status it may ask for is the inbox (M83), which is *less* than the default.
            status: inbox.then_some(cide_ipc::TaskStatus::Inbox),
            // Set at creation rather than by a follow-up edit, for the reason `TaskSink::create`
            // states: the trigger below reads the task this mutation left behind, and a task that
            // did not yet name its change would dispatch a run that is told nothing about the
            // checklist it exists to work through. (M28)
            change: change.cloned(),
            // At creation for the sharper version of the same interleaving: a create naming an
            // assignee dispatches, and a blockedBy edge one call later is a gate the trigger
            // could never have seen. (M30)
            links: if links.is_empty() {
                None
            } else {
                Some(links.to_vec())
            },
            attachments: None,
        };
        let task = self
            .store
            .create(&req, self.author.clone())
            .map_err(|error| error.to_string())?;
        // The files, before the task is answered or the trigger sees it — `cmd::tasks::task_new`'s
        // arrangement for the same reason: a refusal takes the task with it, so a retry does not
        // mint a second `t-<n>` beside a first that has no files. (M39)
        let task = if attachments.is_empty() {
            task
        } else {
            match self.store.attach(
                &task.id,
                AttachTarget::Task,
                attachments,
                self.author.clone(),
            ) {
                Ok(with_files) => with_files,
                Err(error) => {
                    let _ = self.store.delete(&task.id);
                    return Err(error.to_string());
                }
            }
        };
        self.changed.store(true, Ordering::Relaxed);
        self.mutations.lock().push(TaskMutation {
            before: None,
            after: task.clone(),
            author: self.author.clone(),
            // `cide_task_create` naming an assignee is the orchestrator's assignment gesture,
            // exactly as the compose form's dropdown is the user's.
            assign_gesture: agent.is_some(),
            fresh_text: vec![body.to_string()],
        });
        Ok(task)
    }

    fn edit(&self, id: &TaskId, edit: TaskEdit) -> Result<Task, String> {
        // The prose this mutation introduces, read before `edit` is consumed; `before` read
        // immediately ahead of the write — `cmd::tasks::task_edit` documents the race window and
        // why it is accepted.
        let fresh_text: Vec<String> = match &edit {
            TaskEdit::SetBody { body } => vec![body.clone()],
            TaskEdit::Comment { text } => vec![text.clone()],
            _ => Vec::new(),
        };
        // `cide_task_assign` arriving as `Assign { Some }` is an assignment gesture (the
        // orchestrator's revive), the same reading `cmd::tasks::task_edit` gives the dropdown.
        let assign_gesture = matches!(&edit, TaskEdit::Assign { agent: Some(_) });
        let before = self.store.get(id);
        let task = self
            .store
            .edit(id, edit, self.author.clone())
            .map_err(|error| error.to_string())?;
        self.changed.store(true, Ordering::Relaxed);
        self.mutations.lock().push(TaskMutation {
            before,
            after: task.clone(),
            author: self.author.clone(),
            assign_gesture,
            fresh_text,
        });
        Ok(task)
    }

    fn root(&self) -> &Path {
        self.store.root()
    }

    fn search(&self, query: &str) -> Result<Vec<TaskId>, String> {
        Ok(self.store.search(query))
    }

    // The same composite `RegistrySink::integrate` merges, so "unmerged" here and "merged" there
    // are answers about one branch. Read-only, on this connection's thread, as integrate is.
    // (M86)
    fn unmerged(&self, agent: &AgentId, task: &TaskId) -> Result<Option<usize>, String> {
        cide_git::worktree::unmerged(
            self.store.root(),
            &cide_agents::checkout_name(agent, Some(task)),
        )
        .map_err(|error| error.to_string())
    }

    // From the connection's identity, which `app_binder` decided — the same source as the author,
    // so nothing a call carries can make a run look like the orchestrator.
    fn by_run(&self) -> bool {
        matches!(self.author, TaskAuthor::Agent { .. })
    }

    fn cwd(&self) -> &Path {
        &self.cwd
    }

    fn attach(
        &self,
        id: &TaskId,
        target: AttachTarget,
        sources: &[PathBuf],
    ) -> Result<Task, String> {
        // A comment born with its files is prose a mention trigger may scan, exactly as a
        // `TaskEdit::Comment` is in `edit` above.
        let fresh_text: Vec<String> = match &target {
            AttachTarget::NewComment { text } => vec![text.clone()],
            _ => Vec::new(),
        };
        let before = self.store.get(id);
        let task = self
            .store
            .attach(id, target, sources, self.author.clone())
            .map_err(|error| error.to_string())?;
        self.changed.store(true, Ordering::Relaxed);
        self.mutations.lock().push(TaskMutation {
            before,
            after: task.clone(),
            author: self.author.clone(),
            assign_gesture: false,
            fresh_text,
        });
        Ok(task)
    }
}

/// The three orchestration operations that are not simply a registry read.
///
/// # Why these go through the command layer rather than round it
///
/// [`AgentSink::dispatch`] calls `cmd::agents::agents_dispatch` and [`AgentSink::stop`] calls
/// `cmd::agents::agents_stop` — the very functions the Agents panel invokes — instead of talking
/// to [`AgentRegistry`] directly. `agents_dispatch` is where the dispatch refusal lives
/// (`cide_agents::dispatch_refusal`: subagents off for this project, a fault in the role's
/// definition, `bypassPermissions` the project never authorised), where the task is looked up,
/// and where the one-line opening prompt is composed under finding 8's rule. A second path that
/// re-derived any of that would eventually get the `bypassPermissions` arm wrong, which is the
/// one that spawns an unattended process that edits anything it likes. `dispatch_refusal`'s own
/// doc makes this argument: *one function a dispatch site calls*.
///
/// # `block_on`, on a thread that is allowed to block
///
/// Those two are `async fn`s, and this trait is synchronous because the MCP caller is blocked on
/// the answer down a socket. `tauri::async_runtime::block_on` is safe **here** and would not be
/// almost anywhere else: this runs on a per-connection thread ([`serve`]), not on a runtime
/// worker and not on the GTK main loop, so parking it costs this one agent's turn and nothing
/// else. `agents_dispatch`'s own disk work is on `spawn_blocking` inside, which the runtime is
/// still free to service while this thread waits.
///
/// It does **not** wait for the run. `agents_dispatch` enqueues, marks the roster changed, pumps
/// and returns; the fork happens on a task. That is the anti-`openDiff` rule, and it is the
/// reason a wedged subagent cannot freeze the session that dispatched it.
/// [`AgentSink`] over this project's run registry, its dispatch command and its worktrees.
///
/// A second struct rather than four more fields on [`StoreSink`], and the split is the boundary
/// in type form: a run's connection is handed a `&dyn TaskSink` and this value is never built for
/// it, so *what a subagent can reach* is decided by which sink [`ProjectTools::call`] constructs
/// rather than by a check inside a handler that somebody could later remove.
struct RegistrySink {
    app: AppHandle,
    project: ProjectId,
    /// The calling pane's own session — what `notify: here` names. (M40) Out of the connection's
    /// `Scope`, never out of a payload: a model naming a session id would be the
    /// identity-in-the-payload shape the header forbids.
    session: SessionId,
}

impl AgentSink for RegistrySink {
    fn agents(&self) -> Result<Vec<AgentDef>, String> {
        // `project_roster` and not `cide_agents::load_project`, so the `unavailable` sentence a
        // role carries here is the same string the Agents panel is drawing — one wording for one
        // state. It reads the disk on this thread, which is what a per-connection thread is for.
        let roster = crate::cmd::agents::project_roster(&self.app, self.project)
            .ok_or_else(|| gone().to_string())?;
        match roster {
            // An `Err`, deliberately: an empty list would tell the model it has no roles, and a
            // model told that will simply do the work itself and never mention the switch.
            AgentRoster::Disabled { hint, .. } => Err(hint),
            AgentRoster::Empty { .. } => Ok(Vec::new()),
            AgentRoster::Ready { agents, .. } => Ok(agents),
        }
    }

    fn runs(&self) -> Result<Vec<AgentRun>, String> {
        let registry = self
            .app
            .try_state::<Arc<AgentRegistry>>()
            .ok_or_else(|| gone().to_string())?;
        Ok(registry.runs_for(self.project))
    }

    /// Straight from the registry, not from the roster.
    ///
    /// `agents()` above builds an `AgentRoster::Ready` that carries this same flag and threw it
    /// away, which is how the pause stayed invisible to every tool in this file. Asking the
    /// registry is cheaper than rebuilding a roster (no disk, no role parsing) and it is the same
    /// value — `project_roster` fills the field from `AgentRegistry::dispatching`.
    fn dispatching(&self) -> Result<bool, String> {
        let registry = self
            .app
            .try_state::<Arc<AgentRegistry>>()
            .ok_or_else(|| gone().to_string())?;
        Ok(registry.dispatching(self.project))
    }

    // --- what a role runs on (M71) ----------------------------------------------------------
    //
    // Every one of these goes through the `cmd::` command rather than the writer under it, which
    // is `write_definition`'s shape above and for its reason: the command is already the one
    // writer, it carries the validation, and it is what emits. Reaching past it would give this
    // module a second road to the same file, and the two would disagree about exactly the parts
    // nobody tests twice.

    fn config(&self) -> Result<OrchestrationConfig, String> {
        let workspace = self
            .app
            .try_state::<WorkspaceState>()
            .ok_or_else(|| gone().to_string())?;
        tauri::async_runtime::block_on(crate::cmd::agents::agents_config_get(
            workspace,
            self.project,
        ))
        .map_err(|error| error.to_string())
    }

    fn set_config(&self, patch: OrchestrationPatch) -> Result<OrchestrationConfig, String> {
        let workspace = self
            .app
            .try_state::<WorkspaceState>()
            .ok_or_else(|| gone().to_string())?;
        let agents = self
            .app
            .try_state::<Arc<AgentRegistry>>()
            .ok_or_else(|| gone().to_string())?;
        // `agents_config_set` merges into the file that is on disk, so the six disk-only keys a
        // user hand-edited survive a write made from here — and it refuses an *enable* outside a
        // git repository with the sentence naming the two ways out. The handler has already
        // refused `enabled`, so that arm is unreachable through this road; it is left to the
        // command rather than restated, because one refusal is one wording.
        tauri::async_runtime::block_on(crate::cmd::agents::agents_config_set(
            self.app.clone(),
            workspace,
            agents,
            self.project,
            patch,
        ))
        .map_err(|error| error.to_string())
    }

    fn overrides(&self) -> Result<ProjectOverrides, String> {
        let workspace = self
            .app
            .try_state::<WorkspaceState>()
            .ok_or_else(|| gone().to_string())?;
        tauri::async_runtime::block_on(crate::cmd::agents::agent_overrides_get(
            workspace,
            self.project,
        ))
        .map_err(|error| error.to_string())
    }

    fn set_overrides(&self, overrides: ProjectOverrides) -> Result<(), String> {
        let workspace = self
            .app
            .try_state::<WorkspaceState>()
            .ok_or_else(|| gone().to_string())?;
        tauri::async_runtime::block_on(crate::cmd::agents::agent_overrides_set(
            workspace,
            self.project,
            overrides,
        ))
        .map_err(|error| error.to_string())?;
        // **The one emission this road has to make itself.** `agent_overrides_set` emits nothing,
        // which is correct for the panel that calls it — the screen is writing what it is already
        // drawing — and wrong for a write nothing on screen made: `agent-overrides.json` lives in
        // the profile's config directory, which `crate::dotcide` does not watch and nothing else
        // does either, so an override a model wrote would be invisible in every open window until
        // somebody reopened the panel. The roster is what carries a role's override, so the roster
        // is what goes out.
        if let Some(roster) = crate::cmd::agents::project_roster(&self.app, self.project) {
            crate::emit::agents_changed(&self.app, self.project, &roster);
        }
        Ok(())
    }

    fn llm(&self) -> Result<LlmSettings, String> {
        let workspace = self
            .app
            .try_state::<WorkspaceState>()
            .ok_or_else(|| gone().to_string())?;
        workspace
            .with(|ws| Ok::<_, cide_core::CoreError>(ws.settings.llm.clone()))
            .map_err(|error| error.to_string())
    }

    fn pool_load(&self) -> Result<Vec<(cide_ipc::PoolEntry, u32)>, String> {
        let registry = self
            .app
            .try_state::<Arc<AgentRegistry>>()
            .ok_or_else(|| gone().to_string())?;
        Ok(registry.pool_load())
    }

    fn set_llm(&self, llm: LlmSettings) -> Result<(), String> {
        let workspace = self
            .app
            .try_state::<WorkspaceState>()
            .ok_or_else(|| gone().to_string())?;
        // A `SettingsPatch` naming one group, which is the same call the Models screen makes:
        // `apply_patch` touches only the fields a patch names, runs `LlmSettings::cleaned`, and
        // the `cide://workspace-changed` that `WorkspaceState::update` broadcasts is what redraws
        // an open Settings screen. No live run is disturbed, deliberately — an opencode child's
        // configuration is composed at the fork and a running process's environment cannot
        // change, so a provider edited now is honoured from the next dispatch or the next Resume.
        crate::cmd::settings::settings_set(
            workspace,
            self.app.clone(),
            cide_ipc::SettingsPatch {
                llm: Some(llm),
                ..cide_ipc::SettingsPatch::default()
            },
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
    }

    fn resolutions(&self) -> Result<Vec<(AgentId, cide_agents::overrides::Resolved)>, String> {
        crate::agents::resolutions_for(&self.app, self.project).map_err(|error| error.to_string())
    }

    fn milestones_view(&self) -> Result<cide_ipc::MilestonesView, String> {
        crate::milestones::view(&self.app, self.project).ok_or_else(|| gone().to_string())
    }

    fn milestones_define(
        &self,
        mut plan: cide_ipc::MilestonePlan,
    ) -> Result<cide_ipc::MilestonePlan, String> {
        let root = self.root()?;
        // The handler refused a project that has milestones; this is the same rule where the
        // write happens, because a second caller of this method would otherwise be a road
        // around it. (M83)
        if !cide_agents::config::load_milestones(&root).is_empty() {
            return Err(
                "this project already has milestones; they are the user's to change".into(),
            );
        }
        // The tasks each milestone needs are made by `set_plan`, the one road both this tool and
        // the Tasks panel's Milestones tab write through. (M83)
        crate::milestones::set_plan(&self.app, self.project, &mut plan, TaskAuthor::Orchestrator)?;
        crate::milestones::run_gate(&self.app, self.project);
        Ok(plan)
    }

    fn proposals_create(
        &self,
        draft: cide_agents::milestones::ProposalDraft,
    ) -> Result<cide_ipc::Proposal, String> {
        crate::proposals::create(&self.app, self.project, draft, TaskAuthor::Orchestrator)
    }

    fn proposals_list(&self) -> Result<Vec<cide_ipc::Proposal>, String> {
        let root = self.root()?;
        Ok(crate::proposals::list(&self.app, &root))
    }

    fn proposals_withdraw(&self, id: &str) -> Result<(), String> {
        crate::proposals::withdraw(&self.app, self.project, id)
    }

    fn milestones_propose(&self, text: &str) -> Result<TaskId, String> {
        let root = self.root()?;
        let plan = cide_agents::config::load_milestones(&root);
        let task = plan
            .current()
            .and_then(|m| m.task.clone())
            .ok_or("this project has no milestone task to put a proposal on; use `define`")?;
        let stores = self
            .app
            .try_state::<Arc<TasksStores>>()
            .ok_or_else(|| gone().to_string())?;
        let store = stores.ensure(self.project, &root);
        store
            .edit(
                &task,
                TaskEdit::Comment {
                    text: format!(
                        "**Proposed change to the milestones** — for the user to make in \
                         the Milestones tab of the Tasks panel, or to decline.\n\n{}",
                        text.trim()
                    ),
                },
                TaskAuthor::Orchestrator,
            )
            .map_err(|error| error.to_string())?;
        crate::tasks_state::broadcast(&self.app, self.project, &store);
        Ok(task)
    }

    fn definition(&self, scope: AgentScope, name: &AgentId) -> Result<Option<AgentDraft>, String> {
        let root = self.root()?;
        // Straight to `cide_agents::defs`, unlike the write below: `cmd::agents::agents_draft`
        // adds a blocking pool this connection thread does not want and flattens the one
        // distinction that matters here — *there is no such role* against *its file will not
        // parse* — into one `CoreError::Io` sentence.
        match cide_agents::defs::read_draft(&root, scope, name.as_str()) {
            Ok(draft) => Ok(Some(draft)),
            Err(cide_agents::defs::WriteError::NotFound(_)) => Ok(None),
            // A Claude scope resolves a name by **reading the directory** — the file stem need
            // not equal the declared name — so "no file in there declares it" cannot arrive as
            // `NotFound`: `defs::definition_path` answers `None` and `read_draft` turns that into
            // a rejection *about the name*. For a name that is perfectly well-formed that
            // sentence is a lie, so it is an absence here and the handler names the scope.
            Err(cide_agents::defs::WriteError::Rejected(_))
                if scope.is_claude_code()
                    && cide_agents::defs::valid_claude_name(name.as_str()) =>
            {
                Ok(None)
            }
            Err(error) => Err(error.to_string()),
        }
    }

    fn write_definition(&self, draft: &AgentDraft) -> Result<PathBuf, String> {
        let workspace = self
            .app
            .try_state::<WorkspaceState>()
            .ok_or_else(|| gone().to_string())?;
        let agents = self
            .app
            .try_state::<Arc<AgentRegistry>>()
            .ok_or_else(|| gone().to_string())?;
        // Through the command rather than `defs::save` directly, which is `dispatch`'s and
        // `stop`'s shape and for their reason: `agents_save` is already the one writer, and it
        // rebuilds the roster from the directory as it now stands and emits
        // `cide://agents-changed`. Without that emission a role a model wrote would be on disk
        // and absent from every open window — `.cide/` is watched, so a *project* write would
        // arrive eventually through `crate::dotcide`, but `$XDG_CONFIG_HOME/cide/agents/` is
        // watched by nothing at all and a global role would simply never appear.
        match tauri::async_runtime::block_on(crate::cmd::agents::agents_save(
            self.app.clone(),
            workspace,
            agents,
            self.project,
            draft.clone(),
        )) {
            Ok(AgentSaveOutcome::Saved { path, .. }) => Ok(path),
            // The structured problems flattened to the half a model can act on. The field is
            // named *inside* each sentence by the handler, which is where the rendering lives.
            Ok(AgentSaveOutcome::Rejected { problems }) => Err(problems
                .iter()
                .map(|problem| problem.message.as_str())
                .collect::<Vec<_>>()
                .join(" ")),
            Err(error) => Err(error.to_string()),
        }
    }

    fn dispatch(
        &self,
        agent: &AgentId,
        task: Option<&TaskId>,
        instructions: Option<&str>,
        notify: cide_agents::tools::Notify,
    ) -> Result<RunId, String> {
        let (workspace, agents, tasks) = self.dispatch_state()?;
        let request = DispatchRequest {
            project: self.project,
            agent: agent.clone(),
            task: task.cloned(),
            // `prompt` is the extra line, not the task's body: the run is *pointed at* its task
            // and reads it with `cide_task_get`, so nothing multi-line ever reaches the child's
            // terminal — and for a run with no task (M40) it is the whole brief, still one line.
            // `cmd::agents::opening_prompt` flattens whatever arrives here anyway, which is the
            // belt to this braces.
            prompt: instructions.map(str::to_string),
            // `here` is this connection's pane, which only this side of the socket knows. (M40)
            notify: Some(match notify {
                cide_agents::tools::Notify::Here => RunNotify::Session {
                    session: self.session,
                },
                cide_agents::tools::Notify::Main => RunNotify::Primary,
                cide_agents::tools::Notify::None => RunNotify::Silent,
            }),
        };
        tauri::async_runtime::block_on(crate::cmd::agents::agents_dispatch(
            self.app.clone(),
            workspace,
            agents,
            tasks,
            request,
        ))
        .map_err(|error| error.to_string())
    }

    fn stop(&self, run: RunId, reason: Option<&str>, force: bool) -> Result<Stopped, String> {
        // One of three destinations now, and the least valuable of them. (M67) The reason is
        // also typed into the child as part of the wind-down instruction — which is what lets
        // the run's own final comment answer the objection rather than merely describe where it
        // got to — and written onto the task. This line used to say it was the *only* place the
        // reason went, and `AGENT_STOP`'s description named `cide_task_comment` as the durable
        // alternative; both were true and both had to go.
        tracing::info!(
            %run,
            reason = reason.unwrap_or("(none given)"),
            force,
            "the orchestrator stopped a subagent run"
        );
        // The `block_on` stays, and it is not a wait: `agents_stop` returns as soon as the
        // request is accepted, by design. A stop that blocked for the whole grace would be the
        // `openDiff` hazard with a timer on it — a tool call that lets one wedged subagent
        // freeze the product owner mid-turn, when the frozen session is the only thing that
        // could have stopped the others. It would also routinely time out in the client, and a
        // model's move after a failed stop is to stop again, which *is* the escalation to a
        // kill: the blocking design would quietly convert graceful stops into forced ones.
        tauri::async_runtime::block_on(crate::cmd::agents::agents_stop_with(
            self.app.clone(),
            self.project,
            run,
            StopBy::Orchestrator,
            reason.map(str::to_string),
            force,
        ))
        .map_err(|error| error.to_string())
    }

    fn integrate(&self, agent: &AgentId, task: Option<&TaskId>) -> Result<Integrated, String> {
        let workspace = self
            .app
            .try_state::<WorkspaceState>()
            .ok_or_else(|| gone().to_string())?;
        let root = crate::tasks_state::project_root(&workspace, self.project)
            .map_err(|error| error.to_string())?;
        // Straight to `cide-git`, because there is no command in front of it to reuse: the merge
        // has no wire surface yet. It is real work on a real repository and it happens on this
        // connection's thread, which is the one thread in the process that is allowed to wait for
        // it — the caller asked, and the accept loop is untouched. The name is the same
        // composite a dispatch's checkout gets, so the branch merged is by construction the one
        // that run committed to.
        //
        // The milestone checks first (M83): a branch that moves a gate, leaves work uncommitted,
        // or fails the project's verify command is refused here with the reason — this is the
        // model's road, and the panel's Integrate (the user's) does not pass through it.
        let verified =
            crate::milestones::before_integrate(&self.app, self.project, &root, agent, task)?;
        let outcome =
            cide_git::worktree::integrate(&root, &cide_agents::checkout_name(agent, task))
                .map(|outcome| match outcome {
                    cide_git::worktree::Integration::UpToDate => Integrated::UpToDate { verified },
                    cide_git::worktree::Integration::Merged { commit, files } => {
                        Integrated::Merged {
                            commit,
                            files,
                            verified,
                        }
                    }
                    cide_git::worktree::Integration::Conflicts { paths } => {
                        Integrated::Conflicts { paths }
                    }
                })
                .map_err(|error| error.to_string())?;
        // Something landed, so the active milestone's gate is asked again — in the background,
        // because the answer is for whoever plans next, not for this call.
        if matches!(outcome, Integrated::Merged { .. }) {
            crate::milestones::run_gate(&self.app, self.project);
        }
        // And the task's checkout, whose work is now in, is removed — the same rule and the same
        // guards as the panel's Integrate (`agents::retire_worktree`): not while a run stands in
        // it, not while it holds anything the branch does not. (M89) The run that did the work
        // is usually idle at this point, so the removal normally lands when it ends instead.
        if task.is_some()
            && matches!(
                outcome,
                Integrated::Merged { .. } | Integrated::UpToDate { .. }
            )
            && let Some(registry) = self.app.try_state::<Arc<crate::agents::AgentRegistry>>()
        {
            crate::agents::retire_worktree(
                &self.app,
                &registry,
                self.project,
                cide_agents::checkout_name(agent, task),
            );
        }
        Ok(outcome)
    }

    fn now_unix_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_millis() as u64)
            .unwrap_or_default()
    }

    fn isolated(&self) -> Result<bool, String> {
        let workspace = self
            .app
            .try_state::<WorkspaceState>()
            .ok_or_else(|| gone().to_string())?;
        let root = crate::tasks_state::project_root(&workspace, self.project)
            .map_err(|error| error.to_string())?;
        // Read fresh, `nudge_allowed`'s discipline: the file is committed, and a checkout can
        // flip isolation under a running app. One small-file read, on this connection's thread.
        Ok(matches!(
            cide_agents::config::load(&root).agents.isolation,
            cide_agents::Isolation::Worktree
        ))
    }
}

impl RegistrySink {
    /// This project's first root, which is the directory `.cide/` lives in.
    fn root(&self) -> Result<PathBuf, String> {
        let workspace = self
            .app
            .try_state::<WorkspaceState>()
            .ok_or_else(|| gone().to_string())?;
        crate::tasks_state::project_root(&workspace, self.project)
            .map_err(|error| error.to_string())
    }

    /// The three pieces of managed state `agents_dispatch` takes, or the sentence to answer with.
    #[allow(clippy::type_complexity)]
    fn dispatch_state(
        &self,
    ) -> Result<
        (
            tauri::State<'_, WorkspaceState>,
            tauri::State<'_, Arc<AgentRegistry>>,
            tauri::State<'_, Arc<TasksStores>>,
        ),
        String,
    > {
        let workspace = self
            .app
            .try_state::<WorkspaceState>()
            .ok_or_else(|| gone().to_string())?;
        let agents = self
            .app
            .try_state::<Arc<AgentRegistry>>()
            .ok_or_else(|| gone().to_string())?;
        let tasks = self
            .app
            .try_state::<Arc<TasksStores>>()
            .ok_or_else(|| gone().to_string())?;
        Ok((workspace, agents, tasks))
    }
}

/// The sentence for state this process no longer has.
///
/// One wording, because a model reading three different phrasings of "cide is not there any more"
/// has three problems to reason about instead of one. A sentence rather than a JSON-RPC error for
/// [`ToolResult`]'s stated reason: the model can act on this, and it cannot act on an exception.
fn gone() -> &'static str {
    "this project is no longer open in cide, so its subagents cannot be reached"
}

/// How a connection's header becomes the tools it may call.
///
/// A closure rather than an `AppHandle` threaded through [`serve`], so that the handshake, the
/// framing and the notification rule are exercised over a **real socket** by
/// [`tests::a_bridge_gets_a_handshake_a_tool_list_and_an_answer`] with no tauri anywhere —
/// `tauri`'s mock app is behind a feature this build does not enable, so anything holding an
/// `AppHandle` is untestable here. It is `hooks.rs`'s `live_in`/`forget_in` split one level up:
/// the part that can be wrong has to be callable without an app.
///
/// `None` means the connection sent no usable header, which is a scope of its own — see [`serve`].
type Binder = dyn Fn(Option<&Hello>) -> Box<dyn ToolAccess> + Send + Sync;

/// The production binder: resolve the header against the live workspace.
fn app_binder(app: AppHandle) -> Arc<Binder> {
    Arc::new(move |hello: Option<&Hello>| -> Box<dyn ToolAccess> {
        match hello.map_or(Scope::Unscoped, |hello| resolve(&app, hello)) {
            Scope::Pane {
                project, session, ..
            } => Box::new(ProjectTools {
                app: app.clone(),
                project,
                // The nearest honest fit for a session a person is driving in this project, and
                // — the half that is a refusal rather than a label — never `User`, which is the
                // author `TaskStore::edit` lets rewrite and delete somebody else's comment.
                author: TaskAuthor::Orchestrator,
                // Every Claude pane of the project may dispatch (M40); the line this module
                // exists to draw is the `Run` arm below.
                orchestrator: true,
                session: Some(session),
                cwd: None,
            }),
            Scope::Run {
                project,
                agent,
                label,
                cwd,
            } => Box::new(ProjectTools {
                app: app.clone(),
                project,
                // The run's own role, taken from the registry rather than from anything the child
                // said. `TaskEdit::Comment`'s doc is explicit that the author is never a
                // parameter: a caller that could name it could sign a comment as the user.
                author: TaskAuthor::Agent { agent, label },
                // The line this whole module exists to draw.
                orchestrator: false,
                session: None,
                cwd: Some(cwd),
            }),
            Scope::Review {
                run,
                review,
                label,
                harness,
                cwd,
            } => Box::new(crate::mr_review::ReviewTools::new(
                app.clone(),
                run,
                review,
                label,
                harness,
                cwd,
            )),
            Scope::Unscoped => Box::new(NoTools),
        }
    })
}

/// Read one connection: the header, then JSON-RPC until EOF.
fn serve(stream: UnixStream, bind: &Binder) {
    if let Err(error) = stream.set_write_timeout(Some(WRITE_TIMEOUT)) {
        // Without the timeout a frozen agent can park this thread indefinitely. Refusing the
        // connection costs that agent its tools and leaves it a live bridge that answers
        // `-32603`, which is the failure mode `cide_hook::mcp` is written to survive.
        tracing::warn!(%error, "no write timeout on an agent connection; dropping it");
        return;
    }
    let Ok(mut out) = stream.try_clone() else {
        tracing::warn!("could not clone an agent connection for writing; dropping it");
        return;
    };

    let mut lines = BufReader::new(stream).lines();

    // The header first, and exactly once. Blank lines are skipped, matching the bridge's framing.
    let first = loop {
        match lines.next() {
            None => return,
            Some(Err(error)) => {
                tracing::debug!(%error, "an agent connection ended before its header");
                return;
            }
            Some(Ok(line)) if line.trim().is_empty() => continue,
            Some(Ok(line)) => break line,
        }
    };

    // A first line that is not a header is not fatal. It is either an older bridge or something
    // else entirely that found the path; either way it gets an unscoped server rather than a
    // dropped connection, and the line is still served — losing it would hang whatever sent it.
    let (hello, replay) = match parse_hello(&first) {
        Ok(hello) => {
            tracing::debug!(
                pid = hello.pid,
                run = hello.run.as_deref(),
                session = hello.session.as_deref(),
                "an agent bridge connected"
            );
            (Some(hello), None)
        }
        Err(why) => {
            tracing::warn!(%why, "an agent connection sent no usable header; serving no tools");
            (None, Some(first))
        }
    };

    // Bound **once**, here, and never re-derived per message — see the module header on why that
    // is a promise to a client holding a cached `tools/list`.
    let access = bind(hello.as_ref());

    for line in replay.into_iter().map(Ok).chain(lines) {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let Some(reply) = respond(&classify(&line), access.as_ref()) else {
            continue;
        };
        if let Err(error) = write_line(&mut out, &reply) {
            // Fatal to the connection and never retried: a write that failed part-way through a
            // line cannot be resumed, and sending the next reply after a partial one would splice
            // two JSON-RPC messages into one unparseable line. `cide_hook::mcp` states the same
            // rule for the same framing.
            tracing::debug!(%error, "an agent connection went away mid-reply");
            return;
        }
    }
}

/// Parse the header, refusing anything this build cannot read as an identity.
fn parse_hello(raw: &str) -> Result<Hello, String> {
    let hello: Hello = serde_json::from_str(raw).map_err(|error| error.to_string())?;
    if hello.hello != HELLO {
        return Err(format!("not a {HELLO} header"));
    }
    if hello.v != HELLO_VERSION {
        // Refused rather than guessed at. `v` exists so that a field added or removed on the
        // bridge side is a version this end may decline, and an identity read out of a shape we
        // do not know is exactly the thing that must not be guessed — it decides which project's
        // tasks a caller can write.
        return Err(format!("header version {} is not {HELLO_VERSION}", hello.v));
    }
    Ok(hello)
}

/// What this header may reach, decided once per connection.
///
/// The run question first, then the primary-session one. See the module header: a connection that
/// is a run must never be readable as anything else, and the order is what makes that true rather
/// than a coincidence of which ids happen not to collide today.
fn resolve(app: &AppHandle, hello: &Hello) -> Scope {
    if let Some(scope) = resolve_run(app, hello) {
        return scope;
    }
    let Some(state) = app.try_state::<WorkspaceState>() else {
        return Scope::Unscoped;
    };
    state.with(|ws| scope_of(ws, hello))
}

/// The run arm: is this header's `CIDE_RUN` a run this process dispatched?
///
/// The registry is keyed per project and has no global "find this run" — deliberately, because a
/// run id is only ever handed out beside the project it belongs to. So this walks the open
/// projects, which is a handful of maps and happens once per connection, not once per message.
///
/// The workspace lock is taken and released *before* the registry lock, and neither is held
/// across the other: `AgentRegistry`'s module header is explicit that its lock is taken from the
/// hook applier thread, and a connection thread that held the workspace lock while waiting for it
/// would put a third ordering into a pair that has exactly one.
fn resolve_run(app: &AppHandle, hello: &Hello) -> Option<Scope> {
    let run = hello
        .run
        .as_deref()
        .filter(|value| !value.is_empty())?
        .parse::<RunId>()
        .ok()?;
    let registry = app.try_state::<Arc<AgentRegistry>>()?;
    let state = app.try_state::<WorkspaceState>()?;
    let projects: Vec<ProjectId> = state.with(|ws| ws.projects.keys().copied().collect());
    projects
        .into_iter()
        .find_map(|project| run_scope_of(&registry.run_scopes_for(project), run))
}

/// The pure half of [`resolve_run`]: a run id plus one project's runs gives a scope.
///
/// Split out for [`scope_of`]'s reason — this decides which project's tasks a caller can write
/// and under whose name, and a boundary nothing can call without an `AppHandle` is a boundary
/// nothing tests.
fn run_scope_of(runs: &[crate::agents::RunScope], run: RunId) -> Option<Scope> {
    runs.iter()
        .find(|live| live.run == run)
        .map(|live| match &live.purpose {
            crate::agents::RunPurpose::Work => Scope::Run {
                project: live.project,
                agent: live.agent.clone(),
                label: live.label.clone(),
                cwd: live.cwd.clone(),
            },
            // Decided by what the registry says the run was started as, never by its role id:
            // a real role could be called `mr-review` too. See `RunPurpose`.
            crate::agents::RunPurpose::MrReview { review, .. } => Scope::Review {
                run: live.run,
                review: review.clone(),
                label: live.label.clone(),
                harness: live.harness,
                cwd: live.cwd.clone(),
            },
        })
}

/// The pure half of [`resolve`]: a header plus a workspace gives a scope.
///
/// Split out so the rule that decides *which project's tasks a caller can write* is exercised by
/// a unit test rather than only by running the app. It is the security boundary of this module,
/// and a boundary nothing can call without an `AppHandle` is a boundary nothing tests.
fn scope_of(ws: &Workspace, hello: &Hello) -> Scope {
    // Parsed rather than compared as a string: an id we did not mint — a `claude` a user started
    // by hand in a cide shell, which inherits `CIDE_AGENT_SOCK` from its shell — must not be able
    // to match anything by being similarly shaped. `SessionId::from_str` says the same.
    let Some(session) = hello
        .session
        .as_deref()
        .filter(|value| !value.is_empty())
        .and_then(|value| value.parse::<SessionId>().ok())
    else {
        // A header naming only a `run` lands here too, deliberately: this function is the
        // *workspace* half of the decision and a run is not in the workspace. `resolve_run` has
        // already been asked and answered `None`, so an id this process did not mint — or one
        // whose run has been forgotten — gets no tools rather than a guess.
        return Scope::Unscoped;
    };

    // The product owner first, and the order is not arbitrary: `primary_session` is also a
    // Claude pane of its project, so asking the wider question first would answer every console
    // as an ordinary pane. The tools are the same either way since M40; what `primary` still
    // decides is the opening sentence of the paragraph `cmd::session` appends, and a console
    // told it was "one of this project's panes" would be a product owner that does not know it.
    if let Some(project) = project_of_primary(ws, session) {
        return Scope::Pane {
            project,
            session,
            primary: true,
        };
    }

    match project_of_claude_pane(ws, session) {
        Some(project) => Scope::Pane {
            project,
            session,
            primary: false,
        },
        None => Scope::Unscoped,
    }
}

/// The project whose console this session is, if it is any project's.
fn project_of_primary(ws: &Workspace, session: SessionId) -> Option<ProjectId> {
    ws.projects
        .iter()
        .find(|(_, project): &(&ProjectId, &Project)| project.primary_session == session)
        .map(|(id, _)| *id)
}

/// The project one of whose **Claude** panes is showing this session, if any. (M30)
///
/// Detached panes are searched too, and that is a correctness point rather than thoroughness: a
/// conversation pulled out into its own window is `project.detached`, not in any tab's tree, and
/// a caller that scanned only the tabs would take a working agent's tools away the moment
/// somebody dragged its pane out.
///
/// # Why `kind` is checked at all
///
/// A shell pane is given no `CIDE_SESSION` (`cmd::session`'s doing, and `edit_wait` states the
/// same fact from the other side), so a `claude` typed into one announces no session and cannot
/// match here whatever this compares. The check is therefore belt-and-braces — and worth the
/// line, because "which panes may reach a tracker" is the sort of rule that has to be legible in
/// the code rather than inferred from what some other module happens not to set.
///
/// # When the row is written, which decides whether this can be raced
///
/// [`Scope`] is resolved once per connection and never revisited, so a pane whose `session` is
/// not in the workspace *yet* when its bridge connects is a pane with no tools for its whole
/// life. Two cases, and only one of them is even a race:
///
/// * A **task's conversation** — the case this row exists for — is split with
///   `SplitIntent::Resume`, and `cmd::pane::pane_for` writes the session into the row at
///   creation, before `session_spawn` is called at all. There is nothing to race.
/// * A **plain second Claude pane** is created session-less and bound after the spawn resolves,
///   so the binding and the child's bridge do race. The binding is one IPC round trip; the child
///   has a whole CLI to boot before it forks `cide-hook mcp`. It is not a race the child can
///   realistically win, and losing it costs what this pane had before M30 — an empty tool list —
///   rather than anything wrong.
///
/// `cmd::pane`'s `a_resumed_pane_holds_its_session_before_anything_is_spawned` is what keeps the
/// first bullet true from the other side.
fn project_of_claude_pane(ws: &Workspace, session: SessionId) -> Option<ProjectId> {
    ws.projects.iter().find_map(|(id, project)| {
        project
            .tabs
            .iter()
            .flat_map(|tab| tab.tree.panes.values())
            .chain(project.detached.values())
            .any(|pane| pane.kind == PaneKind::Claude && pane.session == Some(session))
            .then_some(*id)
    })
}

// --- JSON-RPC ------------------------------------------------------------------------------------

/// One line from a bridge, classified by envelope only.
#[derive(Debug, PartialEq, Eq)]
enum Inbound {
    /// A method and a non-null id: a reply is owed.
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    /// A method and no usable id: a reply would be a protocol error.
    Notification { method: String },
    /// Not JSON, not an object, or an object that is neither of the above.
    Other,
}

fn classify(raw: &str) -> Inbound {
    let Ok(Value::Object(message)) = serde_json::from_str::<Value>(raw) else {
        return Inbound::Other;
    };
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Inbound::Other;
    };
    match message.get("id") {
        // A null id is treated as no id: JSON-RPC allows it in a request, MCP forbids it, and a
        // reply carrying a null id cannot be matched by anyone — so "expects no reply" is the
        // only safe reading. The bridge classifies it identically; both ends have to agree or a
        // notification comes back as a reply the client cannot place.
        Some(id) if !id.is_null() => Inbound::Request {
            id: id.clone(),
            method: method.to_string(),
            params: message.get("params").cloned().unwrap_or(Value::Null),
        },
        _ => Inbound::Notification {
            method: method.to_string(),
        },
    }
}

/// Answer one message, or `None` when nothing is owed.
///
/// **A notification never gets a reply** — `notifications/initialized` is the one every session
/// sends — and conversely, failing to answer a *request* makes the client wait for ever. Telling
/// the two apart is the one piece of parsing that has to be right on either end of this socket.
fn respond(inbound: &Inbound, access: &dyn ToolAccess) -> Option<Value> {
    let Inbound::Request { id, method, params } = inbound else {
        if let Inbound::Notification { method } = inbound {
            tracing::trace!(%method, "an agent notification, nothing owed");
        }
        return None;
    };

    let reply = match method.as_str() {
        "initialize" => ok(id, initialize_result(params)),
        "tools/list" => ok(id, json!({ "tools": descriptors_for(access) })),
        "tools/call" => call_tool(id, params, access),
        // Answered locally and always: it asks whether this process is alive, and it is.
        "ping" => ok(id, json!({})),
        // Answered rather than fatal. A client SDK probes for capabilities this server does not
        // have — prompts, resources, completions — and closing the connection over one would take
        // the task tools down with it for the rest of the agent's turn.
        other => err(id, METHOD_NOT_FOUND, &format!("no such method: {other}")),
    };
    Some(reply)
}

/// The descriptors this connection is allowed to see.
///
/// An unscoped connection gets `[]` — a real, well-formed empty list rather than an error. The
/// client then shows a server with no tools, which is the honest picture and the one state
/// `cide_hook::mcp` degrades into on its own when cide is not running at all.
fn descriptors_for(access: &dyn ToolAccess) -> Vec<Value> {
    let allowed = access.names();
    // The review vocabulary is a family of its own (M85) and joins the list here, where the
    // per-connection filter already is — one place deciding what a connection sees.
    tools::descriptors()
        .into_iter()
        .chain(cide_agents::review::descriptors())
        .filter(|entry| {
            entry["name"]
                .as_str()
                .is_some_and(|name| allowed.contains(&name))
        })
        .collect()
}

fn call_tool(id: &Value, params: &Value, access: &dyn ToolAccess) -> Value {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return err(id, INVALID_PARAMS, "tools/call without a tool name");
    };
    // A tool this connection cannot see is a JSON-RPC error rather than a result, exactly as in
    // `cide_ide_mcp::server::call_tool`: the caller asked for something that is not in its
    // `tools/list`, which is a mistake about the server rather than about the work. It is also
    // the enforcement half of the scoping — an unscoped connection that guesses a tool name gets
    // nothing, whatever it guesses.
    if !access.names().contains(&name) {
        return err(id, METHOD_NOT_FOUND, &format!("no such tool: {name}"));
    }
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    ok(id, access.call(name, &arguments).to_json())
}

fn initialize_result(params: &Value) -> Value {
    let version = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_PROTOCOL_VERSION);

    json!({
        "protocolVersion": version,
        // `tools` must be present or the client never sends `tools/list` at all. `listChanged` is
        // false and the scope is fixed for the connection's life, which is what makes that claim
        // true — see the module header.
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "cide", "version": env!("CARGO_PKG_VERSION") },
    })
}

fn ok(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn err(id: &Value, code: i32, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// One `write_all` of the message and its newline together.
///
/// Assembled into a single buffer first so a partial write is a partial line rather than a
/// message missing its terminator — the framing rule both ends of this socket share.
fn write_line(sink: &mut impl Write, message: &Value) -> std::io::Result<()> {
    let mut framed = serde_json::to_vec(message)?;
    framed.push(b'\n');
    sink.write_all(&framed)?;
    sink.flush()
}

// ==========================================================================================
// The product-owner loop: telling the orchestrator that a subagent's turn ended.
// ==========================================================================================

/// How long a burst of finished turns is allowed to settle before one line is typed.
///
/// **Two seconds, and deliberately more than an order of magnitude above the roster emit's
/// 120 ms** (`crate::agents::COALESCE`, whose shape this copies). The two windows are sized for
/// two different consumers. The emit keeps a *panel* current, where 120 ms is about the shortest
/// interval a person reads as instant and a longer one reads as lag. This starts a *turn* in
/// somebody's conversation, which costs tokens, occupies the model's attention and cannot be
/// taken back — and the burst it has to collapse is slower besides: six agents "finishing
/// together" finish as each model returns and each `Stop` hook lands, spread over seconds rather
/// than milliseconds. A 120 ms window here would let three of those six through as three separate
/// prompts, which is exactly the failure this exists to prevent.
const NUDGE_COALESCE: Duration = Duration::from_secs(2);

/// And the ceiling, so a project whose runs finish continuously is still told about them.
///
/// Ten seconds, and it bounds the *rate*: a trailing debounce alone never fires while turns keep
/// ending inside its window, so a busy project would be nudged never. Ten rather than the emit's
/// one second for the same reason the debounce is longer — one typed prompt every ten seconds is
/// already about as fast as a product owner can usefully act, and a faster ceiling would spend
/// the orchestrator's context on announcements instead of on the work.
const NUDGE_CEILING: Duration = Duration::from_secs(10);

/// How often the flusher wakes while a burst settles. Short next to [`NUDGE_COALESCE`].
const NUDGE_TICK: Duration = Duration::from_millis(100);

/// How many held turns the coalescer keeps while an orchestrator stays busy.
///
/// A cap because a session can stay mid-turn for minutes while a busy project's runs keep
/// ending, and the buffer summarises to one line however big it grows — 64 is far above any
/// real burst, and dropping the *oldest* keeps the "most recently" the line names true.
const NUDGE_HOLD_CAP: usize = 64;

/// How much of a task title the line may carry, in characters.
///
/// A title is written by whoever created the task — a person, or another model — so it has no
/// length anybody here controls, and a nudge is a prompt: an unbounded title would spend the
/// orchestrator's context on a string it is about to read out of the tracker anyway. The id is
/// the part that matters and is never truncated.
const TITLE_BUDGET: usize = 72;

/// Turns that have ended and not yet been announced. One per process; see [`NudgeCoalescer`].
static NUDGES: NudgeCoalescer = NudgeCoalescer::new();

/// The review tab a task already has open. One per process, beside [`NUDGES`] and for its
/// reason — this is read from the same background thread, and a table found through `try_state`
/// would be a *second* reviewer opened whenever the lookup missed. (M79)
///
/// # The bug this closes, measured on a real board
///
/// Every hand-back opened a tab, and a claude run hands back once per turn — so a task sent
/// back once and reworked twice collected three reviewers, each told by [`review_prompt`] that
/// *nobody else is reviewing it*, which was simply false. On one observed task four of them ran
/// at once and two acted: one dispatched the role afresh while the other, reviewing the same
/// task from the same worktree, posted a contradicting verdict, **stopped a live child** and
/// then had its own dispatch refused three times. Two owners of one task is the shape of that
/// failure, and it is the one thing a reviewer's brief cannot recover from, because each of them
/// is correct about everything it can see.
///
/// So a task has **one** reviewer at a time. A later burst for a task that already has a live
/// one is typed into that conversation instead ([`nudge_line`], the console road's own line),
/// which is both cheaper and better: it is the context that read the task, and it is already
/// holding the thread it would otherwise have to pick up from the comments.
///
/// A `Vec` rather than a map because it holds one entry per *open* review tab — single digits,
/// pruned against the session registry on every read, so a row never outlives the tab a person
/// closed. Keyed on the task, which is what a reviewer owns; a burst whose last turn had no task
/// is never deduplicated, because a task-less run (M40) stands in the project root and two of
/// them have nothing in common to collide over.
static REVIEWERS: Mutex<Vec<Reviewer>> = Mutex::new(Vec::new());

/// One open review tab: the task it owns and the claude in it.
struct Reviewer {
    project: ProjectId,
    task: TaskId,
    session: SessionId,
}

/// The live reviewer for this task, if the tab is still open — pruning every row whose session
/// has gone, which is what a closed tab looks like from here. (M79)
///
/// The liveness question is asked of the [`SessionRegistry`] rather than remembered, for
/// `deliver_nudge`'s reason one step along: a tab can be closed at any moment, including in the
/// two seconds a burst spends settling, and a remembered answer would route a note into a
/// session nobody can read.
fn reviewer_for(app: &AppHandle, project: ProjectId, task: &TaskId) -> Option<SessionId> {
    let sessions = app.try_state::<SessionRegistry>()?;
    let alive = |session: SessionId| sessions.get(session).is_some_and(|pty| !pty.has_exited());
    live_reviewer(&mut REVIEWERS.lock(), project, task, alive)
}

/// The rule, over the table and a liveness answer — pure, so every row of it is a test rather
/// than a claim about a live registry. [`nudge_target`]'s shape, for [`nudge_target`]'s reason.
///
/// It prunes as it reads: a row whose session has gone is a tab somebody closed, and leaving it
/// would suppress that task's next reviewer for the life of the process.
fn live_reviewer(
    open: &mut Vec<Reviewer>,
    project: ProjectId,
    task: &TaskId,
    alive: impl Fn(SessionId) -> bool,
) -> Option<SessionId> {
    open.retain(|reviewer| alive(reviewer.session));
    open.iter()
        .find(|reviewer| reviewer.project == project && &reviewer.task == task)
        .map(|reviewer| reviewer.session)
}

/// Record the tab just opened, so the next burst for the same task finds it.
fn remember_reviewer(project: ProjectId, task: TaskId, session: SessionId) {
    REVIEWERS.lock().push(Reviewer {
        project,
        task,
        session,
    });
}

/// A subagent's turn ended — handed back, finished, or failed. Tell the pane the run's
/// [`RunNotify`] names, once per burst — or nobody, for a run dispatched `notify: none`.
///
/// # Why this is a PTY write at all
///
/// A run reports back only through `.cide/tasks.json`, and nothing reads that file on the
/// orchestrator's behalf. **There is no out-of-band channel into a running `claude`** — no socket,
/// no signal, nothing that hands a live session a message which is not a keystroke — so the only
/// way to tell the project's primary session that there is something new to read is to write into
/// its PTY, exactly as a dispatch writes a run's opening prompt into its child's. That is the
/// third reason the interactive-PTY decision earned its place, and it is also why
/// `AgentsConfig::nudge_orchestrator` exists: this types into somebody's own conversation.
///
/// # Four rules, each a bug if dropped
///
/// 1. **Only when the orchestrator is `Idle | AwaitingInput`** ([`may_be_typed_into`]). Bytes
///    written into a session mid-turn land in whatever it is composing.
/// 2. **One line, terminated with `\r`** ([`submit`]). A PTY write is keystrokes, so an embedded
///    newline is another Enter — finding 8, and the rule every prompt path in this codebase
///    inherits. The bytes are then *delivered* as text-then-lone-Enter through
///    `crate::agents::type_submitted_line`, never one chunk: the TUI's paste detection eats a
///    trailing CR off a long chunk, and the nudge sat unsubmitted in the composer until that
///    was measured — see the helper's header.
/// 3. **Coalesced** ([`NudgeCoalescer`]), or six agents finishing together type six prompts and
///    are answered six times.
/// 4. **The setting is read from disk at the moment of the nudge** ([`deliver_nudge`]), never
///    cached: `.cide/config.json` is committed, and a `git checkout` can switch it off under a
///    running app.
///
/// Called from two producers and nowhere else: `crate::agents::AgentRegistry::set_state`, on
/// the `Running | AwaitingPermission → Idle` edge and on an app-carrying transition into
/// `Finished`/`Failed` — see that method's doc for why the edge and not the state — and
/// `AgentRegistry::watch_exit`'s production closure, which is where a reaped child's exit picks
/// its app back up (the observation itself travels app-free so a test can drive it).
pub fn note_run_over(app: &AppHandle, project: ProjectId, run: RunId) {
    // The death comment first, before the nudge machinery gets any chance to return early: the
    // nudge dedupes per *turn* and can be refused (a busy orchestrator, a repeated edge), and
    // the comment must not inherit either refusal — the board is the durable record, the nudge
    // is a courtesy knock. Its own dedupe is the registry's `death_noted` latch.
    note_death(app, project, run);
    // A run cide ended because its task was done is not news to anybody. See
    // `StopHow::Retired`.
    if app
        .try_state::<Arc<AgentRegistry>>()
        .is_some_and(|registry| registry.was_retired(run))
    {
        return;
    }
    // The facts are read *now*, while they are still true, rather than at flush time: a run can
    // be stopped, finished or forgotten in the two seconds the burst is settling, and a line
    // composed from what the registry says afterwards would describe the wrong thing or nothing.
    // `None` is also a run that asked for silence (M40) — the death comment above was written
    // regardless, because that is board record and this is a knock.
    let Some(turn) = turn_of(app, project, run) else {
        return;
    };
    if !NUDGES.mark(turn) {
        return;
    }
    let app = app.clone();
    // One short-lived thread per burst, as `crate::agents::mark_changed` does it: a process with
    // no subagents in it must not wake up ten times a second for ever.
    if let Err(error) = thread::Builder::new()
        .name("cide-agent-nudge".into())
        .spawn(move || flush_nudges(&app))
    {
        tracing::warn!(%error, "no thread for the orchestrator nudge; this turn ending is not announced");
        // Without this the coalescer stays latched on a flusher that does not exist, and *every
        // later* nudge in the process is swallowed by the `false` return above — one failed
        // thread spawn would silently end the product-owner loop for the life of the app.
        NUDGES.give_up();
    }
}

/// A run that died with a task and no successor writes one comment onto that task.
///
/// P2 of the debug-report plan: the terrastrike runs that died with exit 129 left their tasks
/// in `doing` for ever, and the only evidence was in the Agents panel — which the orchestrator,
/// being a `claude` reading `.cide/tasks.json` over MCP, cannot see. One comment on the task
/// puts the fact where every reader of the board already looks, signed
/// [`TaskAuthor::Orchestrator`] because that is the one author cide itself may write as (the
/// author is never a parameter — `TaskEdit::Comment`'s doc says why).
///
/// Exactly one call site, at the top of [`note_run_over`] — the funnel both producers
/// (`set_state`'s over edge, `watch_exit`'s reap) already end at — so a run reaped twice
/// still comments once: the whole decision, dedupe latch included, is
/// [`AgentRegistry::death_facts`]'s.
fn note_death(app: &AppHandle, project: ProjectId, run: RunId) {
    let Some(registry) = app.try_state::<Arc<AgentRegistry>>() else {
        return;
    };
    let Some(facts) = registry.death_facts(run) else {
        return;
    };
    // `get`, never `ensure`: a project whose tracker is not open has no board on screen and no
    // orchestrator attached to read it, and opening and parsing the file just to write an
    // epitaph into it would be disk work on behalf of nobody. The run's own row still says how
    // it ended.
    let Some(stores) = app.try_state::<Arc<TasksStores>>() else {
        return;
    };
    let Some(store) = stores.get(project) else {
        return;
    };
    let text = epitaph(run, &facts);
    match store.edit(
        &facts.task,
        TaskEdit::Comment { text },
        TaskAuthor::Orchestrator,
    ) {
        Ok(_) => crate::tasks_state::broadcast(app, project, &store),
        // A task deleted between the death and this write is a fine reason to say nothing.
        Err(error) => {
            tracing::debug!(%run, %error, "no death comment; the task is gone or unwritable");
        }
    }
}

/// What one run's ending says on its task. (M67)
///
/// Pure over [`DeathFacts`], so the seven sentences are asserted in this module's tests rather
/// than read out of a live app by somebody who had to stop a real agent to see one.
///
/// # The two old sentences do not move, and that is the disambiguation
///
/// `ended with exit {code}` and `failed` are byte-identical to what they were before M67, and
/// they are what the **unstopped** arms still produce. The point is not that they changed — it
/// is that they now *only* fire when nobody asked, so the reader of an old board and the reader
/// of a new one are told the same thing by the same words. Editing them to mention stops would
/// have re-merged the crash and the stop from the other direction, which is the bug M67 exists
/// to fix.
///
/// House form throughout: a lower-case fragment led by `run {id} ({Role})`, no terminal full
/// stop, and the reason spliced after an em dash — **omitted entirely** when there is none,
/// because `(none given)` belongs in a log and not on somebody's board.
fn epitaph(run: RunId, facts: &DeathFacts) -> String {
    let who = facts.agent_label.as_str();
    let Some(stop) = facts.stop.as_ref() else {
        // Nobody asked. Unchanged, deliberately — see the header.
        return match facts.code {
            Some(code) => {
                format!("run {run} ({who}) ended with exit {code} before finishing this task")
            }
            None => format!("run {run} ({who}) failed before finishing this task"),
        };
    };

    let by = stop.by.phrase();
    // Composed once: every stopped arm ends with it or with nothing.
    let because = match stop
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
    {
        Some(reason) => format!(" — {reason}"),
        None => String::new(),
    };
    let exit = match facts.code {
        Some(code) => format!(" (exit {code})"),
        None => String::new(),
    };

    match &stop.how {
        // Still winding down at the moment its end was observed. Reachable when the child dies
        // of something else mid-wind-down — a crash, an OOM kill — and it is worth its own
        // clause: the run was asked, and what came back is not what was asked for.
        StopHow::WindingDown { .. } => format!(
            "run {run} ({who}) was stopped by {by} and died before it could wind \
             down{exit}{because}"
        ),
        StopHow::WoundDown => format!(
            "run {run} ({who}) was stopped by {by} and wound down as asked{exit}; what it had \
             to say is in its own last comment{because}"
        ),
        StopHow::Forced { why } => format!(
            "run {run} ({who}) was stopped by {by} and {why}, so cide ended it{exit}; anything \
             it had not already written down is gone{because}"
        ),
        // Two spellings, and they must not collapse into one: "we asked and it would not go"
        // is `Forced` above, and this is "we never asked". `why` is `None` when that was the
        // caller's own choice, and names the obstacle when it was not — a stop that silently
        // declined to ask first is a feature that appears not to work.
        StopHow::Immediate { why: None } => format!(
            "run {run} ({who}) was killed outright by {by}, without being asked to wind \
             down{exit}; its in-flight turn is gone{because}"
        ),
        StopHow::Immediate { why: Some(why) } => format!(
            "run {run} ({who}) was stopped by {by} and killed outright{exit}. It was not asked \
             to wind down first: {why}{because}"
        ),
        StopHow::BeforeStart => format!(
            "run {run} ({who}) was stopped by {by} before it started; nothing ran and nothing \
             in this task's worktree changed{because}"
        ),
        StopHow::Discarded => format!(
            "run {run} ({who}) was discarded by {by} without being resumed; its conversation is \
             still on disk, so this task can be dispatched again{because}"
        ),
        // Not a stop anybody asked for — `bring_up` reclaiming a checkout — and until M67 it
        // produced the crash sentence above about a run whose turn had already ended.
        StopHow::Reclaimed => format!(
            "run {run} ({who}) was wound down to give its worktree to another task{exit}; its \
             turn had already ended, so nothing was interrupted"
        ),
        // Unreachable through `note_death`, which `death_facts` refuses for this arm; spelled
        // anyway so the table stays total and a later caller gets a true sentence.
        StopHow::Retired => format!(
            "run {run} ({who}) was ended because its task is done{exit}; its turn had already \
             ended, so nothing was interrupted"
        ),
    }
}

/// A run has been asked to wind down: say so on its task, now. (M67)
///
/// **The one comment that is written before the end**, and only on the one road that has a
/// window. Every other stop is over by the time `AgentRegistry::stop` returns and says
/// everything it has to say in [`epitaph`]; a wind-down takes up to the whole grace, and a cide
/// that quits inside it would take the caller's reason with it. Two comments on one task is a
/// real cost — every comment is tokens in `cide_task_get` — and it is paid only where the run
/// is about to write its *own* comment between the two, so the three read as a conversation:
/// you were asked, here is what I did, here is how it ended.
///
/// `get`, never `ensure` — [`note_death`]'s rule: a project whose tracker is not open has no
/// board on screen and no orchestrator attached to read it, and opening and parsing the file to
/// write into it would be disk work on behalf of nobody.
pub fn note_stop_asked(app: &AppHandle, project: ProjectId, run: RunId) {
    let Some(registry) = app.try_state::<Arc<AgentRegistry>>() else {
        return;
    };
    let Some(asked) = registry.stop_asked_facts(run) else {
        return;
    };
    let Some(stores) = app.try_state::<Arc<TasksStores>>() else {
        return;
    };
    let Some(store) = stores.get(project) else {
        return;
    };
    let because = match asked
        .reason
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
    {
        Some(reason) => format!(" — {reason}"),
        None => String::new(),
    };
    let text = format!(
        "run {run} ({}) was asked to wind down by {}, with {}s to finish and report{because}",
        asked.agent_label,
        asked.by.phrase(),
        asked.grace_secs,
    );
    match store.edit(
        &asked.task,
        TaskEdit::Comment { text },
        TaskAuthor::Orchestrator,
    ) {
        Ok(_) => crate::tasks_state::broadcast(app, project, &store),
        Err(error) => {
            tracing::debug!(%run, %error, "no stop comment; the task is gone or unwritable");
        }
    }
}

/// A session's turn ended — if nudges were held for a busy orchestrator, try again.
///
/// Called from the hook applier's `Effect::State` arm for **every** session transition in the
/// process, which is why the shape is two cheap refusals and a latch: the state filter costs
/// nothing, and [`NudgeCoalescer::rearm`] is one lock and an `is_empty` in the overwhelmingly
/// common no-holds case — the applier thread's ordering is a correctness requirement and
/// nothing here may make it wait. It does not know (and must not look up — that is a workspace
/// lock) whether `state`'s session is even the right project's product owner; the flusher
/// re-resolves and re-checks everything at delivery, and a wrong-session re-arm merely holds
/// again. The retry rate is bounded by real turn-end edges plus the coalescer's own debounce,
/// so nothing spins.
pub fn note_session_ready(app: &AppHandle, state: SessionState) {
    if !may_be_typed_into(state) {
        return;
    }
    if !NUDGES.rearm() {
        return;
    }
    let app = app.clone();
    if let Err(error) = thread::Builder::new()
        .name("cide-agent-nudge".into())
        .spawn(move || flush_nudges(&app))
    {
        tracing::warn!(%error, "no thread to redeliver held nudges; they wait for the next edge");
        // Unlatch `flushing` only — unlike `note_run_over`'s give_up, the held turns are kept:
        // the next ready edge or the next `mark` re-arms, and the facts are not lost to one
        // failed spawn.
        NUDGES.state.lock().flushing = false;
    }
}

/// Wait the burst out, then write at most one line per (project, route).
fn flush_nudges(app: &AppHandle) {
    loop {
        thread::sleep(NUDGE_TICK);
        let Some(due) = NUDGES.take_due() else {
            continue;
        };
        for ((project, route), turns) in by_route(due) {
            deliver_nudge(app, project, &route, turns);
        }
        return;
    }
}

/// Read the setting, find the pane the route names, check it is not mid-turn, and type the line.
///
/// Every step can decline, and each declines by doing nothing: this runs on a background thread
/// with no user in front of it, and there is no screen on which "the nudge could not be sent"
/// would be a useful sentence. The orchestrator is never left without the information — asking
/// `mcp__cide__cide_agent_runs` answers the same question, and answers it as of the moment it is
/// called.
fn deliver_nudge(app: &AppHandle, project: ProjectId, route: &NudgeRoute, turns: Vec<Turn>) {
    let Some(workspace) = app.try_state::<WorkspaceState>() else {
        return;
    };
    let Ok(root) = crate::tasks_state::project_root(&workspace, project) else {
        return;
    };

    // Read once, here, and off the disk — `nudge_allowed`'s rule applied to the whole block
    // rather than to one key of it. `.cide/config.json` is committed, so a teammate's commit or
    // a `git checkout` can change any of this under a running app, and a value cached at
    // dispatch would announce a run the way somebody had already stopped asking for.
    let config = cide_agents::config::load(&root).agents;

    if !config.nudge_orchestrator {
        tracing::debug!(%project, "the orchestrator nudge is off for this project");
        return;
    }

    // The fork. (M79)
    //
    // **Above `nudge_target` and above `may_be_typed_into`, and both omissions are deliberate.**
    // Those two gates exist because the console road writes into a conversation somebody else
    // is using: it has to find which one, and it has to wait until that one is not mid-turn.
    // This road creates the conversation it writes into, so there is nothing to find and nothing
    // to wait for — and, correspondingly, nothing to *hold*: a burst that cannot be delivered
    // now goes back into the coalescer, and a burst that opens its own tab has already been
    // delivered.
    //
    // It **replaces** the console line rather than joining it. Two announcements of one fact
    // cost two turns and give two readers the same job, and the reader with the empty context is
    // the one this feature exists to reach.
    if config.finish_in_new_tab {
        // **One reviewer per task**, and the second burst is told rather than duplicated —
        // [`REVIEWERS`] carries the whole argument and the board it was measured on. Asked
        // before the prompt is built, because the two roads say different things: a tab that
        // does not exist yet needs the brief, and one that is already reading this task needs
        // the news.
        let task = turns.last().and_then(|turn| turn.task.clone());
        if let Some(session) = task
            .as_ref()
            .and_then(|task| reviewer_for(app, project, task))
        {
            return note_to_reviewer(app, project, session, turns);
        }
        let prompt = review_prompt(
            config.review_prompt_template(),
            &turns,
            title_of(app, project, &turns).as_deref(),
        );
        match crate::claude_tab::open_with_prompt(
            app,
            project,
            &review_tab_title(&turns),
            None,
            &prompt,
            // The project's own stance for an unattended child, read from the same file in the
            // same breath as the switch that opened this tab. A reviewer that parks on a
            // permission prompt nobody will answer is the failure `permission_mode` documents,
            // and its brief tells it not to edit anything — `TabMode`'s doc carries the
            // trade, because this one stands in the project root rather than a worktree.
            crate::claude_tab::TabMode {
                unattended: config.unattended(),
                // Behind what the user is reading: see the field.
                behind: true,
            },
            // The worktree the run under review worked in, so the reviewer starts *in* the
            // branch — and so its transcript files beside that work instead of in the project's
            // own Claude history, which is the reported bug. `None` for a task-less run, which
            // had no checkout of its own (M40), and the helper falls back to the root for a
            // worktree that has since gone.
            review_checkout(&root, &turns),
        ) {
            Ok((session, tab)) => {
                tracing::info!(%project, %session, %tab, turns = turns.len(), "a finished run opened its own reviewer");
                // After the tab exists and never before: a row naming a session that failed to
                // spawn would suppress the *next* burst's tab for a reviewer that never opened.
                if let Some(task) = task {
                    remember_reviewer(project, task, session);
                }
            }
            // Dropped rather than held, and never silently: holding would wait for a condition
            // that does not exist (this road has no busy session to wait on), and falling back
            // to the console road would type into the pane the project asked cide not to use.
            // The death comment `note_death` already wrote is the durable record either way.
            Err(error) => {
                tracing::warn!(%project, %error, "no review tab for a finished run; nobody was told");
            }
        }
        return;
    }

    // Resolved *here*, at delivery, for both arms: the primary because it moves, the named pane
    // because it may have closed since the dispatch — and a burst that spent two seconds
    // settling is two seconds in which either can have happened.
    let alive = |session: SessionId| {
        app.try_state::<SessionRegistry>()
            .and_then(|sessions| sessions.get(session))
            .is_some_and(|pty| !pty.has_exited())
    };
    let Some(session) = workspace.with(|ws| nudge_target(ws, project, route, alive)) else {
        return;
    };

    // Rule 1, and asked *here* rather than when the edge fired: the two seconds the burst spent
    // settling are two seconds in which the product owner may have started a turn of its own.
    let Some(hooks) = app.try_state::<crate::hooks::HookServer>() else {
        return;
    };
    let state = hooks.state(session);
    if !may_be_typed_into(state) {
        // **Held, not dropped.** This used to drop, on the theory that the next run to finish
        // would nudge again — which is false for the *last* run of a burst: an orchestrator
        // mid-turn while its final subagent handed back simply never heard, and the loop ended
        // with the work done and nobody told. So the turns go back into the coalescer and
        // [`note_session_ready`] re-arms a flusher on this session's next `Idle`/`AwaitingInput`
        // edge; delivery re-checks everything then, so a held nudge is never staler than the
        // moment it is finally typed — and the line points at `cide_agent_runs`, which answers
        // as of *now*, whatever it says. Every other decline above still drops: a project that
        // said no must not be typed at later, and a project with no session has nowhere to hold
        // for.
        tracing::debug!(%project, ?state, "the product owner is busy; holding the nudge");
        NUDGES.hold(turns);
        return;
    }

    let title = title_of(app, project, &turns);
    let Some(bytes) = submit(&nudge_line(&turns, title.as_deref())) else {
        return;
    };

    let Some(pty) = app
        .try_state::<SessionRegistry>()
        .and_then(|sessions| sessions.get(session))
    else {
        return;
    };
    tracing::info!(%project, turns = turns.len(), "nudging the product owner");
    // The text now, the Enter alone after a beat — never one chunk. The TUI's paste detection
    // is length-triggered, and a line this long written whole lands in the composer with its
    // CR eaten: the measured shape was two nudges stacked in the product owner's input box,
    // submitted by nobody. `type_submitted_line`'s header carries the measurements.
    crate::agents::type_submitted_line(app, session, &pty, bytes);
}

/// Tell the review tab that already owns this task, instead of opening a second one. (M79)
///
/// It gets [`nudge_line`] — the console road's own line — and not [`review_prompt`]: this
/// conversation was given the brief when it opened, has read the task, and is very likely
/// holding the thread the new turn continues. Repeating the whole brief would spend its context
/// re-stating what it already knows and read as a *different* job to do.
///
/// The two gates the tab road deliberately omits both come back here, because this road has
/// what that one lacks — a conversation somebody else is already using. Mid-turn it **holds**,
/// for the console road's reason: the reviewer's own next `Idle` edge re-arms the flusher
/// through [`note_session_ready`], so the news waits rather than landing in whatever the
/// reviewer is composing.
fn note_to_reviewer(app: &AppHandle, project: ProjectId, session: SessionId, turns: Vec<Turn>) {
    let Some(hooks) = app.try_state::<crate::hooks::HookServer>() else {
        return;
    };
    let state = hooks.state(session);
    if !may_be_typed_into(state) {
        tracing::debug!(%project, %session, ?state, "the task's reviewer is mid-turn; holding");
        NUDGES.hold(turns);
        return;
    }
    let title = title_of(app, project, &turns);
    let Some(bytes) = submit(&nudge_line(&turns, title.as_deref())) else {
        return;
    };
    let Some(pty) = app
        .try_state::<SessionRegistry>()
        .and_then(|sessions| sessions.get(session))
    else {
        return;
    };
    tracing::info!(%project, %session, turns = turns.len(), "telling the task's open reviewer");
    crate::agents::type_submitted_line(app, session, &pty, bytes);
}

/// The worktree the run being reviewed stood in, if it had one. (M79)
///
/// **The same composite a dispatch's checkout gets**, through `cide_agents::checkout_name` and
/// `cide_git::worktree::path_of` — the two producers that built the directory in the first place
/// — so the reviewer is by construction standing in the checkout that run committed to, exactly
/// as `RegistrySink::integrate` merges by construction the branch it committed to.
///
/// `None` when the run had no task: such a run works in the project root (M40) and has no
/// checkout of its own, so there is nothing to stand in that the root is not.
///
/// This reads the *last* turn of the burst, which is the one the prompt is about — the same
/// choice `title_of` makes beside it, and for the same reason.
fn review_checkout(root: &std::path::Path, turns: &[Turn]) -> Option<std::path::PathBuf> {
    let turn = turns.last()?;
    let task = turn.task.as_ref()?;
    let name = cide_agents::checkout_name(&turn.agent, Some(task));
    Some(cide_git::worktree::path_of(root, &name))
}

/// The title of the task the most recent ended turn was for, if there is one and it is known.
///
/// One producer for both roads (M79). The console line and the review tab name the same task, so
/// two lookups would be two chances for them to name it differently — and the second would be
/// taken a few milliseconds later, which is exactly long enough for a run to have renamed it.
fn title_of(app: &AppHandle, project: ProjectId, turns: &[Turn]) -> Option<String> {
    turns
        .last()
        .and_then(|turn| turn.task.as_ref())
        .and_then(|task| task_title(app, project, task))
}

/// One ended turn, as the edge recorded it.
///
/// The facts are copied rather than referenced back to the registry for `AgentRun::agent_label`'s
/// reason, one step further along: a line that said what the run *is* by the time it is typed
/// would name a role that has since been renamed, or nothing at all for a run the registry has
/// already dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Turn {
    project: ProjectId,
    /// Whose conversation the line goes into — copied off the run at the edge, for the reason
    /// the label is: what the run *asked for* at dispatch, not what a later lookup would say.
    route: NudgeRoute,
    /// The role's **id**, which is what a dispatch names and what its checkout is built from.
    ///
    /// Carried beside the label rather than derived from it, because they are different strings
    /// and only one of them is a name anything answers to: a role labelled `3D Artist` has the
    /// id `3d-artist`, and `cide_agents::checkout_name` builds `cide/3d-artist-t-1060` from the
    /// id and a slug of the task. The first cut of the review prompt interpolated the label and
    /// told a reviewer to diff `cide/3D Artist-t-1060` — a branch that has never existed, with a
    /// space in it, which `cide_git::worktree`'s own whitelist would refuse outright.
    agent: AgentId,
    agent_label: String,
    task: Option<TaskId>,
    outcome: TurnOutcome,
}

/// Where a turn's line is typed. (M40) [`RunNotify`] with the silent arm removed — a silent run
/// never becomes a [`Turn`] at all, so the coalescer holds only turns that go somewhere.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum NudgeRoute {
    /// The project's primary pane, resolved at delivery: `Project::primary_session` moves on
    /// every console restart, so it is a role here and a session only when the line is typed.
    Primary,
    /// The pane that dispatched or assigned. Falls back to the primary at delivery if it is gone
    /// — see [`nudge_target`].
    Session(SessionId),
}

/// How the turn ended — the difference between "read what it wrote and answer it" and "that
/// role's child is gone".
///
/// For a `claude` run every ordinary turn is [`Self::HandedBack`] (the child stays at its prompt
/// between turns and exits only when stopped); for an `opencode` run one turn is one process, so
/// [`Self::Finished`] is its *normal* end — which is why the nudge could not stay
/// handed-back-only without one whole harness's runs ending silently.
#[derive(Debug, Clone, PartialEq, Eq)]
enum TurnOutcome {
    /// The child is alive at its prompt, waiting to be answered or retried.
    HandedBack,
    /// The child exited; the code is the reaper's, never invented.
    Finished { code: i32 },
    /// The run failed before or instead of finishing — a spawn that never came up, a stop.
    Failed,
}

/// What the registry can still say about the run whose turn just ended.
///
/// The outcome is read from the run's state *now*, immediately after the transition that
/// triggered this — `Finished`/`Failed` runs are retained by the registry precisely so a late
/// reader still finds them.
///
/// `None` for a run that asked for silence (M40) — said in the log, because "nothing happened"
/// is otherwise indistinguishable from the nudge being off, and the two have different fixes.
fn turn_of(app: &AppHandle, project: ProjectId, run: RunId) -> Option<Turn> {
    let registry = app.try_state::<Arc<AgentRegistry>>()?;
    registry
        .runs_for(project)
        .into_iter()
        .find(|live| live.run == run)
        .and_then(|live| {
            let route = match live.notify {
                RunNotify::Primary => NudgeRoute::Primary,
                RunNotify::Session { session } => NudgeRoute::Session(session),
                RunNotify::Silent => {
                    tracing::debug!(%run, "the run asked for no announcement; nothing is typed");
                    return None;
                }
            };
            Some((live, route))
        })
        .map(|(live, route)| Turn {
            project,
            route,
            agent: live.agent,
            agent_label: live.agent_label,
            task: live.task,
            outcome: match live.state {
                RunState::Finished { code } => TurnOutcome::Finished { code },
                RunState::Failed { .. } => TurnOutcome::Failed,
                // The handed-back edge left the run `Idle`; any other state means it has already
                // moved on (a retry re-entered `Running` inside the burst window), and the honest
                // summary of the edge that fired is still "a turn ended".
                _ => TurnOutcome::HandedBack,
            },
        })
}

/// A task's current title, if this project's tracker is already open.
///
/// Deliberately **not** `TasksStores::ensure`: this is a decoration on a prompt, and opening and
/// parsing a file for it would be disk work on the way to a nicety. A project whose tracker has
/// never been read gets a line naming the task id, which is the part the orchestrator needs in
/// order to go and read it.
fn task_title(app: &AppHandle, project: ProjectId, task: &TaskId) -> Option<String> {
    let stores = app.try_state::<Arc<TasksStores>>()?;
    let store = stores.get(project)?;
    store.get(task).map(|task| task.title)
}

/// Which session states may be typed into.
///
/// `Idle` and `AwaitingInput` and nothing else. `Busy` and `AwaitingPermission` are a turn in
/// flight, and bytes written into one land in whatever the CLI is composing; `Spawning` and
/// `Splash` are a session that has not finished starting, so the keystrokes would be eaten by a
/// screen that is about to be replaced; `Paused` is a `SIGSTOP`ped child, where a write succeeds
/// into the kernel's buffer and is read on resume out of order with whatever it was doing — the
/// same hazard `AgentRegistry::pause` documents; and `Exited` has nothing behind it at all.
///
/// The unknown case resolves here too, and safely: `HookServer::state` answers `Spawning` for a
/// session it has never seen, which is every session in a build whose hook socket failed to bind.
/// Losing the nudge is the right way to be wrong about that.
fn may_be_typed_into(state: SessionState) -> bool {
    matches!(state, SessionState::Idle | SessionState::AwaitingInput)
}

/// Group a burst by (project, route), first-seen order, so each pane is typed at once. (M40)
///
/// The route is part of the key because one project's burst may now be owed to two panes — the
/// console and the pane that dispatched — and one line naming both would be typed into one of
/// them about work the other is waiting on.
fn by_route(turns: Vec<Turn>) -> Vec<((ProjectId, NudgeRoute), Vec<Turn>)> {
    let mut grouped: Vec<((ProjectId, NudgeRoute), Vec<Turn>)> = Vec::new();
    for turn in turns {
        let key = (turn.project, turn.route.clone());
        match grouped.iter_mut().find(|(other, _)| *other == key) {
            Some((_, bucket)) => bucket.push(turn),
            None => grouped.push((key, vec![turn])),
        }
    }
    grouped
}

/// Which session a route's line is typed into, given what is still standing. (M40)
///
/// `Primary` is the project's `primary_session` as it is *now*. `Session` is that pane if it is
/// alive (`alive` — the registry's answer, injected so this is testable without one) **and**
/// still a Claude pane of this project ([`project_of_claude_pane`], the same predicate that scoped
/// its connection); otherwise the primary, with a line in the log — the fact is not dropped for
/// want of its first address, and a pane the user closed mid-run is exactly the case where the
/// console is the next place they will look. `None` only when the project itself is gone.
fn nudge_target(
    ws: &Workspace,
    project: ProjectId,
    route: &NudgeRoute,
    alive: impl Fn(SessionId) -> bool,
) -> Option<SessionId> {
    let primary = cide_core::workspace::project(ws, project)
        .ok()
        .map(|project| project.primary_session)?;
    match route {
        NudgeRoute::Primary => Some(primary),
        NudgeRoute::Session(session) => {
            if alive(*session) && project_of_claude_pane(ws, *session) == Some(project) {
                Some(*session)
            } else {
                tracing::debug!(
                    %project, %session,
                    "the pane that dispatched is gone; announcing to the primary pane instead"
                );
                Some(primary)
            }
        }
    }
}

/// The line itself: what happened, and where to look. **One line, always.**
///
/// Short on purpose. The session already knows what the tools are — `cmd::session`'s roster
/// paragraph named every one of them at spawn — so restating the vocabulary here would spend the
/// orchestrator's context repeating its own system prompt. What it cannot know is that something
/// has just changed, and which task changed it.
///
/// Composed from what actually moved, and then flattened as a whole by [`one_line`]. The
/// flattening is not decoration: an agent label comes from a committed markdown file and a task
/// title from a tracker any agent may write, so both are strings this process did not choose. A
/// newline in either would be a second Enter and would submit the tail of this sentence as a
/// second turn — finding 8, and `the_nudge_is_one_line_whatever_the_task_is_called` is the guard.
fn nudge_line(turns: &[Turn], title: Option<&str>) -> String {
    let last = turns.last();
    let who = last
        .map(|turn| turn.agent_label.as_str())
        .map(one_line)
        .filter(|label| !label.is_empty())
        .map_or_else(|| "a subagent".to_string(), |label| format!("`{label}`"));

    // The outcome is the difference between "answer it" and "its child is gone": a claude run
    // hands every ordinary turn back and exits only when stopped, an opencode run *finishes* as
    // its normal end — the orchestrator's next move differs, so the line says which.
    let ended = match last.map(|turn| &turn.outcome) {
        None | Some(TurnOutcome::HandedBack) => "handed its turn back".to_string(),
        Some(TurnOutcome::Finished { code }) => format!("finished (exit {code})"),
        Some(TurnOutcome::Failed) => "failed".to_string(),
    };

    let task = last.and_then(|turn| turn.task.as_ref());
    let on = match (task, title.map(one_line).filter(|title| !title.is_empty())) {
        (Some(id), Some(title)) => format!(" on task {id} ({})", clip(&title)),
        (Some(id), None) => format!(" on task {id}"),
        (None, _) => String::new(),
    };

    let line = match (turns.len(), task.is_some()) {
        (0 | 1, true) => format!(
            "A subagent turn just ended: {who} {ended}{on}. Read that task's comments and check \
             mcp__cide__cide_agent_runs before deciding what to do next."
        ),
        (0 | 1, false) => format!(
            "A subagent turn just ended: {who} {ended}{on}, dispatched with no task. Check \
             mcp__cide__cide_agent_runs."
        ),
        (many, _) => format!(
            "{many} subagent turns just ended, most recently {who} ({ended}){on}. Check \
             mcp__cide__cide_agent_runs and the tasks they commented on."
        ),
    };
    one_line(&line)
}

/// What the review tab is called. (M79)
///
/// The role and the task, because a session that opens five of these over an afternoon needs to
/// tell them apart in the strip, and those are the two facts that differ. Clipped through
/// [`clip`] for its reason: a task title is written by whoever created the task — a person, or
/// another model — so it is a string this process did not choose and must not let set the width
/// of a tab.
fn review_tab_title(turns: &[Turn]) -> String {
    let Some(last) = turns.last() else {
        return "Review".to_string();
    };
    let who = one_line(&last.agent_label);
    let who = if who.is_empty() { "subagent" } else { &who };
    match &last.task {
        Some(task) => clip(&format!("Review: {who} · {task}")),
        None => clip(&format!("Review: {who}")),
    }
}

/// What the review tab's `claude` is told. **One line, always.** (M79)
///
/// # Why this is longer than [`nudge_line`] rather than the same sentence
///
/// `nudge_line` is short on purpose, and its doc says why: the console session already knows
/// what the tools are, because `cmd::session`'s roster paragraph named every one of them in its
/// system prompt at spawn. Restating the vocabulary would spend that context repeating itself.
///
/// **A review tab knows none of that.** Its conversation is empty, and — the part that decides
/// the shape of this string — **nobody else is going to finish the job.** The console is not
/// being told; that is what `finishInNewTab` means. So this cannot be a request for an opinion:
/// a verdict written into a comment that nothing reads is the same loop ending in *and then the
/// user happens to notice* that the whole feature exists to close.
///
/// So the tab **owns the task from here**. It has exactly two ways to put it down:
///
/// * satisfied — comment the verdict, **merge the branch** with `cide_agent_integrate`, and
///   set the task `done`. The merge is not an afterthought and is the part that was missing
///   first: accepting work means taking it into the branch, and a reviewer that said *"the
///   branch isn't merged yet; I left that to you"* had handed the job back to the one person
///   this feature exists to keep out of the loop. `integrate` computes the merge in memory and
///   refuses on conflict without writing a byte, so the failure arm is safe to fold into the
///   hand-back road; or
/// * not satisfied — comment what is left **and hand the task back to the role that did it**,
///   with `cide_agent_dispatch` carrying that feedback as the run's brief.
///
/// The second arm is what makes this a loop rather than a report, and it closes on itself: the
/// re-dispatched run ends, opens another review tab, and that one reads the comments this one
/// wrote. Which is also the escape hatch — the comments are the record, so a reviewer that can
/// see the task has already been sent back for the same reason is told to stop and leave it for
/// a person. That is the only bound available, and it is the right one: cide cannot judge
/// whether a third attempt is progress, and a loop counter in Rust would stop a task that was
/// genuinely converging.
///
/// It names the tools by their exact `mcp__cide__…` handles and nothing else about them — one
/// line of prompt against a paragraph of description that would go stale — and it names the
/// role by **id**, because that is what a dispatch answers to.
///
/// # The branch
///
/// [`cide_agents::checkout_name`] and never a `format!` here: a run's checkout is
/// `cide/<role-id>-<slug of the task id>`, where the slug is the task id forced through the
/// worktree grammar. The first cut spelled it by hand from the *label*, and told a reviewer to
/// diff `cide/3D Artist-t-1060` — a branch with a space in it that has never existed and that
/// `cide_git::worktree`'s whitelist would refuse. One producer, and the composite is only
/// meaningful for a run that had a task: a task-less run stands in the project root (M40).
///
/// Flattened as a whole by [`one_line`], for [`nudge_line`]'s reason and with more at stake:
/// this string is built from an agent label out of a committed markdown file and a task title
/// out of a tracker any agent may write, and a newline in either is a second Enter that would
/// submit the first half of the instructions as its own turn.
///
/// # The template (M84)
///
/// The task case is the project's `agents.reviewPrompt`, filled by
/// [`cide_agents::config::fill_review_prompt`] — editable per project in Settings under the
/// switch that opens this tab. The text below the switch ships as
/// [`cide_agents::config::DEFAULT_REVIEW_PROMPT`], and every clause argued above is in it. The
/// task-less case stays here: it has no task, branch or role to fill a template with.
fn review_prompt(template: &str, turns: &[Turn], title: Option<&str>) -> String {
    let last = turns.last();
    let who = last
        .map(|turn| turn.agent_label.as_str())
        .map(one_line)
        .filter(|label| !label.is_empty())
        .map_or_else(|| "a subagent".to_string(), |label| format!("`{label}`"));

    let ended = match last.map(|turn| &turn.outcome) {
        None | Some(TurnOutcome::HandedBack) => "handed its turn back".to_string(),
        Some(TurnOutcome::Finished { code }) => format!("finished (exit {code})"),
        Some(TurnOutcome::Failed) => "failed".to_string(),
    };

    let also = match turns.len() {
        0 | 1 => String::new(),
        many => format!(
            " {} other subagent turn(s) ended in the same moment; mcp__cide__cide_agent_runs \
             lists them and each needs the same treatment.",
            many - 1
        ),
    };

    let Some(turn) = last else {
        return one_line(
            "A subagent turn just ended and cide could not say which. Check \
             mcp__cide__cide_agent_runs and the tasks they commented on.",
        );
    };

    let line = match turn.task.as_ref() {
        Some(id) => {
            let title = title
                .map(one_line)
                .filter(|title| !title.is_empty())
                .map(|t| clip(&t));
            let named = match &title {
                Some(title) => format!("task {id} ({title})"),
                None => format!("task {id}"),
            };
            let branch = format!("cide/{}", cide_agents::checkout_name(&turn.agent, Some(id)));
            // `who` without its backticks: the template puts them where it wants them.
            let agent = who.trim_matches('`');
            cide_agents::config::fill_review_prompt(
                template,
                &[
                    ("task_id", &id.0),
                    ("task_title", title.as_deref().unwrap_or("")),
                    ("task", &named),
                    ("branch", &branch),
                    ("role", &turn.agent.0),
                    ("agent", agent),
                    ("outcome", &ended),
                    ("others", &also),
                ],
            )
        }
        None => format!(
            "A subagent just {ended}: {who}, dispatched with no task, so it stood in the project \
             root and has no branch of its own. You are reviewing it with fresh context and \
             nobody else is, and nobody will answer a question — decide yourself from what you \
             can read. Check mcp__cide__cide_agent_runs for what it was asked to do, then \
             look at the working tree with git status and git diff to see what it left behind. \
             Say plainly whether that is what was wanted. There is no task to record this on, so \
             if something is wrong say what, and what you would dispatch to fix it.{also}"
        ),
    };
    one_line(&line)
}

/// Whatever it is handed, on one line, with runs of whitespace collapsed.
///
/// `split_whitespace` is what does the work and it is chosen for covering `\r` as well as `\n`:
/// a lone carriage return in a task title is *also* an Enter to a PTY, and a filter that only
/// looked for `\n` would leave the more obscure half of the bug in place.
pub(crate) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A title, at most [`TITLE_BUDGET`] characters, with an ellipsis when it was cut.
///
/// `chars`, not bytes: a byte slice of a UTF-8 title panics on a boundary, and a panic in a
/// background thread aborts the process under `panic = "abort"`.
fn clip(title: &str) -> String {
    if title.chars().count() <= TITLE_BUDGET {
        return title.to_string();
    }
    let kept: String = title.chars().take(TITLE_BUDGET).collect();
    format!("{}…", kept.trim_end())
}

/// The bytes to write, or `None` if this build cannot compose them.
///
/// Routed through the harness rather than spelled `format!("{line}\r")` here, so that the nudge
/// and a dispatch's opening prompt are the **same bytes built by the same function**: the `\r`
/// rule (finding 8 — a PTY write is keystrokes, so the terminator is Enter and an embedded
/// newline is another one) then has one implementation in the process rather than one per prompt
/// path. `Harness::Claude` because the session being typed into is a project's primary console
/// pane, which is a `claude`; `Delivery::Respawn` is unreachable for it and is dropped rather
/// than guessed at.
pub(crate) fn submit(line: &str) -> Option<Vec<u8>> {
    match cide_agents::for_kind(Harness::Claude)?.deliver(line) {
        Delivery::Stdin(bytes) => Some(bytes),
        Delivery::Respawn => None,
    }
}

/// A trailing debounce with a ceiling over the turns that have ended, modelled line for line on
/// `crate::agents::Coalescer` — same fields, same `mark`/`take_due` split, longer windows.
///
/// # Why a `static` and not managed state
///
/// The edge that feeds this is `AgentRegistry::set_state`, which is reached from the hook applier
/// thread, and a coalescer found through `try_state` would be a nudge silently dropped whenever
/// the lookup missed. It holds no resource and no handle — an empty `Vec` and three `Option`s —
/// so a process that never dispatches a subagent carries a mutex it never locks.
struct NudgeCoalescer {
    state: Mutex<NudgeState>,
}

/// A `Mutex` and not atomics, for `crate::agents::CoalesceState`'s reason: `first` and `last` have
/// to move together or the ceiling and the debounce disagree about the same burst.
struct NudgeState {
    /// The turns that have ended and not yet been announced, in the order they ended.
    ///
    /// A `Vec` rather than the emit coalescer's `HashSet<ProjectId>`, because this one carries
    /// *facts* — six turns of one project are one prompt that says six, and which role and task
    /// finished last is the only concrete thing the line has to say.
    pending: Vec<Turn>,
    /// When the first un-announced turn of this burst ended. Drives [`NUDGE_CEILING`].
    first: Option<Instant>,
    /// And the most recent. Drives [`NUDGE_COALESCE`].
    last: Option<Instant>,
    /// Whether a flusher thread is already waiting this burst out.
    flushing: bool,
}

impl NudgeCoalescer {
    const fn new() -> Self {
        Self {
            state: Mutex::new(NudgeState {
                pending: Vec::new(),
                first: None,
                last: None,
                flushing: false,
            }),
        }
    }

    /// Record a turn. `true` means *you are the thread that must flush it*.
    fn mark(&self, turn: Turn) -> bool {
        let mut state = self.state.lock();
        let now = Instant::now();
        state.pending.push(turn);
        state.first.get_or_insert(now);
        state.last = Some(now);
        if state.flushing {
            return false;
        }
        state.flushing = true;
        true
    }

    /// The turns to announce, or `None` while the burst is still arriving.
    fn take_due(&self) -> Option<Vec<Turn>> {
        let mut state = self.state.lock();
        let (first, last) = (state.first?, state.last?);
        if last.elapsed() < NUDGE_COALESCE && first.elapsed() < NUDGE_CEILING {
            return None;
        }
        state.first = None;
        state.last = None;
        state.flushing = false;
        Some(std::mem::take(&mut state.pending))
    }

    /// Unlatch after a flusher that could not be started. See [`note_run_over`].
    fn give_up(&self) {
        let mut state = self.state.lock();
        state.pending.clear();
        state.first = None;
        state.last = None;
        state.flushing = false;
    }

    /// Put a busy project's turns back, to be flushed when its session next hands its own turn
    /// back. See the busy arm of [`deliver_nudge`] for why holding beat dropping.
    ///
    /// The held turns go to the *front*, ahead of anything that ended while delivery was being
    /// decided, so the line's "most recently" stays honest; the clock is reset so the ceiling
    /// cannot fire the moment a flusher is re-armed; and the buffer is capped — past
    /// [`NUDGE_HOLD_CAP`] the oldest are dropped, because the line only ever summarises a count
    /// and points at `cide_agent_runs`, which answers as of the moment it is called.
    fn hold(&self, turns: Vec<Turn>) {
        let mut state = self.state.lock();
        let mut pending = turns;
        pending.extend(std::mem::take(&mut state.pending));
        if pending.len() > NUDGE_HOLD_CAP {
            pending.drain(..pending.len() - NUDGE_HOLD_CAP);
        }
        state.pending = pending;
        let now = Instant::now();
        state.first = Some(now);
        state.last = Some(now);
        state.flushing = false;
    }

    /// Re-arm a flusher for held turns. `true` means *you must start the flusher thread* —
    /// [`Self::mark`]'s latch, for the redelivery path.
    ///
    /// The empty case is the overwhelming one (every session state change in the process asks),
    /// and it costs exactly one lock and one `is_empty` — which is what lets
    /// [`note_session_ready`] sit on the ordered hook-applier thread.
    fn rearm(&self) -> bool {
        let mut state = self.state.lock();
        if state.pending.is_empty() || state.flushing {
            return false;
        }
        let now = Instant::now();
        state.first.get_or_insert(now);
        state.last.get_or_insert(now);
        state.flushing = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_agents::config::DEFAULT_REVIEW_PROMPT;
    use cide_core::workspace;
    use cide_ipc::TaskStatus;

    fn hello(session: Option<&str>, run: Option<&str>) -> Hello {
        Hello {
            hello: HELLO.to_string(),
            v: HELLO_VERSION,
            run: run.map(str::to_string),
            session: session.map(str::to_string),
            pid: Some(4242),
        }
    }

    fn workspace_with_a_project() -> (Workspace, ProjectId, SessionId) {
        let mut ws = Workspace::default();
        let root = std::env::temp_dir().join(format!("cide-agent-rpc-{}", std::process::id()));
        let project = workspace::open_project(&mut ws, vec![root], None).expect("open a project");
        let primary = ws.projects[&project].primary_session;
        (ws, project, primary)
    }

    /// One pane row, of the shape `cmd::pane::pane_split` writes.
    ///
    /// Built by hand rather than through that command, which needs an `AppHandle` — and what
    /// these tests are about is the row [`project_of_claude_pane`] reads, not how it got there.
    fn a_pane(kind: cide_ipc::PaneKind, session: SessionId) -> cide_ipc::Pane {
        cide_ipc::Pane {
            id: cide_ipc::PaneId::new(),
            kind,
            role: cide_ipc::PaneRole::Auxiliary,
            session: Some(session),
            conversation: None,
            conversation_since: None,
            continues: None,
            harness: None,
            title: "cide : claude".into(),
            docker: None,
        }
    }

    /// Split a second pane into this project's console tab, as `openSession.ts` does.
    fn split_into_console(
        ws: &mut Workspace,
        project: ProjectId,
        kind: cide_ipc::PaneKind,
        session: SessionId,
    ) {
        let pane = a_pane(kind, session);
        let tab = &mut ws.projects[&project].tabs[0];
        tab.tree.panes.insert(pane.id, pane);
    }

    /// One line, captured verbatim from the real `cide-hook mcp` on this machine.
    ///
    /// This is where the two halves of the bridge actually meet: everything else about the header
    /// is asserted against a shape *this* file believes in, and a shape both ends believe in
    /// separately is how a contract drifts. `cide_hook::mcp::header_line` builds it and this
    /// parses it, and the only evidence that those are the same document is a byte string one of
    /// them produced. Re-capture it by running `cide-hook mcp` with `CIDE_AGENT_SOCK` pointing at
    /// any listener and reading the first line.
    const A_REAL_HEADER: &str = r#"{"hello":"cide-mcp","v":1,"run":null,"session":"5f2b1c60-1111-4222-8333-444455556666","pid":711639}"#;

    #[test]
    fn a_header_must_be_one_this_build_knows() {
        let real = parse_hello(A_REAL_HEADER).expect("the real bridge's header parses");
        assert_eq!(real.pid, Some(711639));
        assert_eq!(
            real.session.as_deref(),
            Some("5f2b1c60-1111-4222-8333-444455556666")
        );
        assert_eq!(real.run, None);

        assert!(
            parse_hello(r#"{"hello":"cide-mcp","v":1,"run":null,"session":null,"pid":1}"#).is_ok()
        );
        // A future bridge is refused rather than read as though it were this one: the header
        // decides which project's tasks the caller can write.
        assert!(parse_hello(r#"{"hello":"cide-mcp","v":2,"session":null}"#).is_err());
        assert!(parse_hello(r#"{"hello":"something-else","v":1}"#).is_err());
        assert!(parse_hello("not json at all").is_err());
        // A JSON-RPC message is not a header, which is what makes the fall-through in `serve`
        // reachable rather than theoretical.
        assert!(parse_hello(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#).is_err());
    }

    #[test]
    fn a_claude_pane_of_a_project_is_what_scopes_and_the_console_is_marked_primary() {
        let (ws, project, primary) = workspace_with_a_project();

        assert_eq!(
            scope_of(&ws, &hello(Some(&primary.to_string()), None)),
            Scope::Pane {
                project,
                session: primary,
                primary: true
            }
        );

        // A session bound to no pane at all — a `claude` the user started by hand in a cide
        // shell, which inherits `CIDE_AGENT_SOCK` but is given no `CIDE_SESSION`, or an id
        // simply invented — resolves to nothing. This is the security boundary of the module,
        // and the row added in M30 does not move it: being *in the workspace* is the test, and a
        // caller cannot put itself there.
        let stranger = SessionId::new().to_string();
        assert_eq!(
            scope_of(&ws, &hello(Some(&stranger), None)),
            Scope::Unscoped
        );

        // A run names nothing this slice can resolve, so it gets no tools rather than a guess.
        assert_eq!(
            scope_of(
                &ws,
                &hello(None, Some("11111111-1111-4111-8111-111111111111"))
            ),
            Scope::Unscoped
        );

        // Neither, empty, and unparseable all land in the same place.
        assert_eq!(scope_of(&ws, &hello(None, None)), Scope::Unscoped);
        assert_eq!(scope_of(&ws, &hello(Some(""), None)), Scope::Unscoped);
        assert_eq!(
            scope_of(&ws, &hello(Some("developer"), None)),
            Scope::Unscoped
        );
    }

    /// The row M28 needed and M18's table did not have.
    ///
    /// `TasksPanel/openSession.ts` opens a task's conversation as an ordinary second Claude pane
    /// and `spec_dispatch_to_session` types `opening_prompt` into it — a sentence naming
    /// `mcp__cide__cide_task_get`. Before this row that pane was served an empty `tools/list`,
    /// and the model, told to use a tool it had not been shown, hand-wrote `.cide/tasks.json`
    /// from a guessed schema. Nothing here fails loudly if the row is removed again: the pane
    /// still connects, still draws as connected, and the damage happens in somebody's repo.
    #[test]
    fn a_task_conversation_pane_reaches_its_own_projects_tracker() {
        let (mut ws, project, primary) = workspace_with_a_project();

        let conversation = SessionId::new();
        split_into_console(&mut ws, project, PaneKind::Claude, conversation);
        assert_eq!(
            scope_of(&ws, &hello(Some(&conversation.to_string()), None)),
            Scope::Pane {
                project,
                session: conversation,
                primary: false
            }
        );

        // The console is unmoved, and that is what the ordering inside `scope_of` buys: the
        // primary pane is *also* a Claude pane of this project, so asking the wider question
        // first would answer every console as an ordinary pane — same tools since M40, but a
        // product owner whose paragraph no longer tells it so.
        assert_eq!(
            scope_of(&ws, &hello(Some(&primary.to_string()), None)),
            Scope::Pane {
                project,
                session: primary,
                primary: true
            }
        );

        // The pane's *kind* is part of the rule. A shell pane is given no `CIDE_SESSION`, so
        // this is unreachable in production — asserted so the check cannot later be tidied away
        // as redundant on the grounds that nothing exercises it.
        let shell = SessionId::new();
        split_into_console(&mut ws, project, PaneKind::Shell, shell);
        assert_eq!(
            scope_of(&ws, &hello(Some(&shell.to_string()), None)),
            Scope::Unscoped
        );
    }

    /// A conversation dragged out into its own window is still that project's.
    ///
    /// `detach_pane` moves the row out of the tab's tree and into `Project::detached`, so a
    /// lookup that walked only the tabs would take a working agent's tools away mid-turn at the
    /// moment somebody dragged its pane out — and the connection's scope is fixed for its life,
    /// so re-docking would not give them back.
    #[test]
    fn a_detached_conversation_pane_is_still_its_projects() {
        let (mut ws, project, _) = workspace_with_a_project();
        let conversation = SessionId::new();
        let pane = a_pane(PaneKind::Claude, conversation);
        ws.projects[&project].detached.insert(pane.id, pane);

        assert_eq!(
            scope_of(&ws, &hello(Some(&conversation.to_string()), None)),
            Scope::Pane {
                project,
                session: conversation,
                primary: false
            }
        );
    }

    #[test]
    fn two_projects_do_not_see_each_others_trackers() {
        let (mut ws, first, first_primary) = workspace_with_a_project();
        let second_root = std::env::temp_dir().join("cide-agent-rpc-second");
        let second = workspace::open_project(&mut ws, vec![second_root], None).expect("second");
        let second_primary = ws.projects[&second].primary_session;

        assert_eq!(
            scope_of(&ws, &hello(Some(&first_primary.to_string()), None)),
            Scope::Pane {
                project: first,
                session: first_primary,
                primary: true
            }
        );
        assert_eq!(
            scope_of(&ws, &hello(Some(&second_primary.to_string()), None)),
            Scope::Pane {
                project: second,
                session: second_primary,
                primary: true
            }
        );
        assert_ne!(first, second);

        // And a conversation pane is scoped to the project whose console it was split into, not
        // to whichever project the walk reaches first.
        let in_second = SessionId::new();
        split_into_console(&mut ws, second, PaneKind::Claude, in_second);
        assert_eq!(
            scope_of(&ws, &hello(Some(&in_second.to_string()), None)),
            Scope::Pane {
                project: second,
                session: in_second,
                primary: false
            }
        );
    }

    fn a_run(project: ProjectId, agent: &str) -> crate::agents::RunScope {
        crate::agents::RunScope {
            run: RunId::new(),
            project,
            agent: AgentId(agent.to_string()),
            label: format!("{agent} (label)"),
            cwd: PathBuf::from(format!("/repo/.cide/worktrees/{agent}")),
            purpose: crate::agents::RunPurpose::Work,
            harness: cide_ipc::Harness::Claude,
        }
    }

    #[test]
    fn a_run_scopes_to_its_own_project_and_signs_as_its_own_role() {
        let project = ProjectId::new();
        let runs = vec![a_run(project, "developer"), a_run(project, "qa")];

        assert_eq!(
            run_scope_of(&runs, runs[1].run),
            Some(Scope::Run {
                project,
                agent: AgentId("qa".into()),
                // The label comes from the registry, never from the child: a run that could name
                // its own author could sign a comment as the user. See `TaskEdit::Comment`.
                label: "qa (label)".into(),
                // And so does the cwd: it is the worktree the app started the child in, which is
                // what a relative path in `cide_task_attach` is read against. (M39)
                cwd: PathBuf::from("/repo/.cide/worktrees/qa"),
            })
        );

        // A run id this process never minted resolves to nothing, which is `Scope::Unscoped` at
        // the call site and therefore no tools at all.
        assert_eq!(run_scope_of(&runs, RunId::new()), None);
        assert_eq!(run_scope_of(&[], runs[0].run), None);
    }

    #[test]
    fn a_review_run_is_scoped_to_its_review_and_never_to_the_tracker() {
        let project = ProjectId::new();
        let mut review = a_run(project, "mr-review");
        review.purpose = crate::agents::RunPurpose::MrReview {
            review: "r1".into(),
            cwd: PathBuf::from("/cache/review/source"),
            brief: String::new(),
            tools: Vec::new(),
            harness: Harness::Opencode,
        };
        review.harness = Harness::Opencode;
        let runs = vec![a_run(project, "developer"), review.clone()];
        // Decided by the purpose the registry holds, not the role id: a real role named
        // `mr-review` is an ordinary run.
        assert!(matches!(
            run_scope_of(&runs, runs[0].run),
            Some(Scope::Run { .. })
        ));
        assert_eq!(
            run_scope_of(&runs, review.run),
            Some(Scope::Review {
                run: review.run,
                review: "r1".into(),
                label: review.label.clone(),
                harness: Harness::Opencode,
                cwd: review.cwd.clone(),
            })
        );
        // And a connection answering the review names lists those six and nothing else.
        let access = FakeAccess {
            names: cide_agents::review::tool::REVIEW.to_vec(),
            calls: Mutex::new(Vec::new()),
        };
        let listed: Vec<String> = descriptors_for(&access)
            .iter()
            .map(|d| d["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(listed, cide_agents::review::tool::REVIEW);
        assert!(!listed.iter().any(|n| n.starts_with("cide_task_")));
    }

    /// Records every call, so the JSON-RPC layer can be driven with no app and no store.
    struct FakeAccess {
        names: Vec<&'static str>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeAccess {
        fn all() -> Self {
            Self {
                names: tools::tool::ALL.to_vec(),
                calls: Mutex::new(Vec::new()),
            }
        }

        /// The product owner's list: the nine task tools and the eleven orchestration ones.
        fn every() -> Self {
            Self {
                names: tools::tool::EVERY.to_vec(),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn none() -> Self {
            Self {
                names: Vec::new(),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl ToolAccess for FakeAccess {
        fn names(&self) -> &[&'static str] {
            &self.names
        }

        fn call(&self, name: &str, _arguments: &Value) -> ToolResult {
            self.calls.lock().push(name.to_string());
            ToolResult::text("ok")
        }
    }

    fn ask(raw: &str, access: &dyn ToolAccess) -> Option<Value> {
        respond(&classify(raw), access)
    }

    #[test]
    fn a_notification_is_never_answered_and_a_request_always_is() {
        let access = FakeAccess::all();
        assert!(
            ask(
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                &access
            )
            .is_none()
        );
        // A null id is a notification, on both ends of this socket.
        assert!(ask(r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#, &access).is_none());
        assert!(ask("garbage", &access).is_none());
        assert!(ask(r#"{"jsonrpc":"2.0","id":7,"result":{}}"#, &access).is_none());

        let reply = ask(r#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#, &access).expect("a reply");
        assert_eq!(reply["id"], json!(7));
        assert_eq!(reply["result"], json!({}));
    }

    #[test]
    fn initialize_echoes_the_clients_version_and_advertises_tools() {
        let access = FakeAccess::none();
        let reply = ask(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
            &access,
        )
        .expect("a reply");
        assert_eq!(reply["result"]["protocolVersion"], json!("2024-11-05"));
        assert_eq!(
            reply["result"]["capabilities"]["tools"]["listChanged"],
            json!(false)
        );

        // Even with no tools: the handshake must be clean, or the client shows a crashed server
        // at every launch instead of a server with nothing in it.
        let default = ask(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
            &access,
        )
        .expect("a reply");
        assert_eq!(
            default["result"]["protocolVersion"],
            json!(DEFAULT_PROTOCOL_VERSION)
        );
    }

    #[test]
    fn an_unscoped_connection_lists_nothing_and_can_call_nothing() {
        let access = FakeAccess::none();
        let listed =
            ask(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#, &access).expect("a reply");
        assert_eq!(listed["result"]["tools"], json!([]));

        // And guessing a name it was never shown gets it nowhere. This is the enforcement half of
        // the scoping: the empty list is not merely cosmetic.
        let called = ask(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"cide_task_create","arguments":{"title":"x"}}}"#,
            &access,
        )
        .expect("a reply");
        assert_eq!(called["error"]["code"], json!(METHOD_NOT_FOUND));
        assert!(
            access.calls.lock().is_empty(),
            "nothing may reach the store"
        );
    }

    #[test]
    fn a_scoped_connection_lists_the_task_tools_and_calls_them() {
        let access = FakeAccess::all();
        let listed =
            ask(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#, &access).expect("a reply");
        let names: Vec<&str> = listed["result"]["tools"]
            .as_array()
            .expect("an array")
            .iter()
            .filter_map(|entry| entry["name"].as_str())
            .collect();
        assert_eq!(names, tools::tool::ALL);

        let called = ask(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"cide_task_list","arguments":{}}}"#,
            &access,
        )
        .expect("a reply");
        assert_eq!(called["result"]["isError"], json!(false));
        assert_eq!(*access.calls.lock(), vec!["cide_task_list".to_string()]);
    }

    #[test]
    fn an_unknown_method_or_a_nameless_call_is_answered_rather_than_fatal() {
        let access = FakeAccess::all();
        let unknown = ask(
            r#"{"jsonrpc":"2.0","id":4,"method":"resources/list"}"#,
            &access,
        )
        .expect("a reply");
        assert_eq!(unknown["error"]["code"], json!(METHOD_NOT_FOUND));

        let nameless = ask(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{}}"#,
            &access,
        )
        .expect("a reply");
        assert_eq!(nameless["error"]["code"], json!(INVALID_PARAMS));

        let stranger = ask(
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"rm_rf"}}"#,
            &access,
        )
        .expect("a reply");
        assert_eq!(stranger["error"]["code"], json!(METHOD_NOT_FOUND));
    }

    /// A JSON-RPC client over a real unix socket, so the handshake is exercised end to end.
    struct Client {
        out: UnixStream,
        lines: std::io::Lines<BufReader<UnixStream>>,
    }

    impl Client {
        fn connect(path: &Path) -> Self {
            let out = UnixStream::connect(path).expect("connect");
            let read = out.try_clone().expect("clone for reading");
            // Bounded, so a server that never answers fails this test instead of hanging CI on
            // it — which is the failure mode a socket test is most likely to introduce.
            read.set_read_timeout(Some(Duration::from_secs(10)))
                .expect("read timeout");
            Self {
                out,
                lines: BufReader::new(read).lines(),
            }
        }

        fn send(&mut self, message: Value) {
            eprintln!("→ {message}");
            write_line(&mut self.out, &message).expect("write");
        }

        fn recv(&mut self) -> Value {
            let line = self
                .lines
                .next()
                .expect("a reply is owed")
                .expect("readable");
            eprintln!("← {line}");
            serde_json::from_str(&line).expect("one json object per line")
        }
    }

    /// The whole protocol over a real socket: header, `initialize`, a notification that must not
    /// be answered, `tools/list`, `tools/call` — and a second connection whose header resolves to
    /// nothing, which must get a clean handshake and no tools at all.
    ///
    /// Everything else in this module tests a function; this tests the *sequence*, which is where
    /// an unanswered request or an answered notification would actually live. Run it with
    /// `cargo test -p cide-app agent_rpc -- --nocapture` to read the exchange.
    #[test]
    fn a_bridge_gets_a_handshake_a_tool_list_and_an_answer() {
        let path = std::env::temp_dir().join(format!(
            "cide-agent-rpc-live-{}-{:?}.sock",
            std::process::id(),
            thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind");

        // Stands in for `app_binder`: the header decides, exactly as it does in production, with
        // no workspace to decide against.
        let bind: Arc<Binder> = Arc::new(|hello: Option<&Hello>| -> Box<dyn ToolAccess> {
            match hello.and_then(|hello| hello.session.as_deref()) {
                Some("the-console") => Box::new(FakeAccess::all()),
                _ => Box::new(FakeAccess::none()),
            }
        });

        let server = thread::spawn(move || {
            for stream in listener.incoming().take(3) {
                serve(stream.expect("accept"), bind.as_ref());
            }
        });

        {
            let mut console = Client::connect(&path);
            console.send(json!({
                "hello": HELLO, "v": HELLO_VERSION,
                "run": null, "session": "the-console", "pid": 4242,
            }));

            console.send(json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": { "protocolVersion": "2025-06-18" },
            }));
            let init = console.recv();
            assert_eq!(init["id"], json!(1));
            assert_eq!(init["result"]["serverInfo"]["name"], json!("cide"));

            // The notification must draw no reply at all. Asserted by what comes back *next*: if
            // it were answered, the ping's id would not be the first thing on the wire, and the
            // real client would be left holding a reply it cannot match.
            console.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
            console.send(json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }));
            assert_eq!(console.recv()["id"], json!(2));

            console.send(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }));
            let listed = console.recv();
            let names: Vec<&str> = listed["result"]["tools"]
                .as_array()
                .expect("an array")
                .iter()
                .filter_map(|entry| entry["name"].as_str())
                .collect();
            assert_eq!(names, tools::tool::ALL);

            console.send(json!({
                "jsonrpc": "2.0", "id": 4, "method": "tools/call",
                "params": { "name": "cide_task_list", "arguments": {} },
            }));
            let called = console.recv();
            assert_eq!(called["id"], json!(4));
            assert_eq!(called["result"]["isError"], json!(false));
        }

        {
            let mut stranger = Client::connect(&path);
            stranger.send(json!({
                "hello": HELLO, "v": HELLO_VERSION,
                "run": null, "session": "somebody-elses-session", "pid": 4243,
            }));
            stranger.send(json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {},
            }));
            assert_eq!(stranger.recv()["id"], json!(1));

            stranger.send(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
            assert_eq!(stranger.recv()["result"]["tools"], json!([]));

            stranger.send(json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": { "name": "cide_task_create", "arguments": { "title": "x" } },
            }));
            assert_eq!(stranger.recv()["error"]["code"], json!(METHOD_NOT_FOUND));
        }

        {
            // A client that sends no header at all — an older bridge, or something else that
            // found the path. It gets an unscoped server, and the line it *did* send is still
            // answered: dropping it would leave whoever sent it waiting for ever, which is the
            // one failure this socket exists to avoid.
            let mut headerless = Client::connect(&path);
            headerless.send(json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {},
            }));
            let init = headerless.recv();
            assert_eq!(init["id"], json!(1));
            assert_eq!(init["result"]["serverInfo"]["name"], json!("cide"));

            headerless.send(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
            assert_eq!(headerless.recv()["result"]["tools"], json!([]));
        }

        server.join().expect("the server thread");
        let _ = std::fs::remove_file(&path);
    }

    /// The scoping table from the module header, over a real socket, in one exchange.
    ///
    /// This is the test the whole module exists for. Everything else here checks a function; this
    /// checks the thing a reviewer actually wants to know — *can a subagent dispatch a subagent* —
    /// through the transport, the header line and the JSON-RPC layer, exactly as `cide-hook mcp`
    /// drives them.
    ///
    /// Three connections, three answers: twenty-two tools for the project's primary session, nine for
    /// a run, none for anything else. And the enforcement half beside the advertisement: the run
    /// connection asks for `cide_agent_dispatch` by name and is answered `METHOD_NOT_FOUND`
    /// **without the call reaching the sink at all**, which is the assertion that matters — an
    /// empty `tools/list` that a guessed name could walk around would be cosmetic.
    ///
    /// Run it with `cargo test -p cide-app agent_rpc -- --nocapture` to read the exchange.
    #[test]
    fn a_run_gets_the_task_tools_and_can_never_reach_a_dispatch() {
        let path = std::env::temp_dir().join(format!(
            "cide-agent-rpc-scope-{}-{:?}.sock",
            std::process::id(),
            thread::current().id()
        ));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind");

        // Stands in for `app_binder`, and reproduces its **order**: the run is asked about first,
        // so a header carrying both a run and the console's session is a run. See `resolve`.
        let bind: Arc<Binder> = Arc::new(|hello: Option<&Hello>| -> Box<dyn ToolAccess> {
            match hello {
                Some(hello) if hello.run.as_deref().is_some_and(|run| !run.is_empty()) => {
                    Box::new(FakeAccess::all())
                }
                Some(hello) if hello.session.as_deref() == Some("the-console") => {
                    Box::new(FakeAccess::every())
                }
                _ => Box::new(FakeAccess::none()),
            }
        });

        let server = thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                serve(stream.expect("accept"), bind.as_ref());
            }
        });

        {
            let mut owner = Client::connect(&path);
            owner.send(json!({
                "hello": HELLO, "v": HELLO_VERSION,
                "run": null, "session": "the-console", "pid": 4242,
            }));
            owner.send(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }));
            let names = tool_names(owner.recv());
            assert_eq!(names.len(), 22, "{names:?}");
            assert_eq!(names, tools::tool::EVERY);
            assert!(names.contains(&"cide_task_list"));
            assert!(names.contains(&"cide_agent_dispatch"));
            // The settings four reach a product owner's pane and nowhere else. (M71)
            assert!(names.contains(&"cide_agents_config"));
            assert!(names.contains(&"cide_llm_pool"));
            // The three that are deliberately not tools, checked here as well as in
            // `cide_agents::tools`, because this is the list a model actually receives.
            assert!(!names.contains(&"cide_agent_pause"));
            assert!(!names.contains(&"cide_agent_resume"));
            assert!(!names.contains(&"cide_agent_delete"));
            assert!(!names.contains(&"cide_llm_test_model"));
        }

        {
            let mut run = Client::connect(&path);
            run.send(json!({
                "hello": HELLO, "v": HELLO_VERSION,
                "run": "11111111-1111-4111-8111-111111111111",
                // Deliberately the console's session as well: a run's child carries both
                // variables, and the run must win whatever the session says.
                "session": "the-console", "pid": 4243,
            }));

            run.send(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }));
            let names = tool_names(run.recv());
            assert_eq!(names.len(), 9, "{names:?}");
            assert_eq!(names, tools::tool::ALL);
            for orchestration in tools::tool::ORCHESTRATION {
                assert!(
                    !names.contains(orchestration),
                    "{orchestration} must never be advertised to a run"
                );
            }

            // The task tools do work here — a run's whole job is to report on its task.
            run.send(json!({
                "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "cide_task_comment",
                            "arguments": { "id": "t-1", "text": "worktree merged clean" } },
            }));
            assert_eq!(run.recv()["result"]["isError"], json!(false));

            // And the one that must not. A JSON-RPC error rather than a `ToolResult`, because the
            // caller asked for something that is not in its own `tools/list` — a mistake about the
            // server, not about the work, and the same distinction `cide_ide_mcp` draws.
            run.send(json!({
                "jsonrpc": "2.0", "id": 3, "method": "tools/call",
                "params": { "name": "cide_agent_dispatch",
                            "arguments": { "agent": "developer", "task": "t-1" } },
            }));
            let refused = run.recv();
            assert_eq!(refused["error"]["code"], json!(METHOD_NOT_FOUND));
            assert!(refused["result"].is_null(), "{refused}");

            // And the settings tools, guessed by name. A subagent that could re-point its
            // siblings' models — or raise the project's own concurrency cap — is the failure the
            // family split exists to make unreachable, so it is asserted where the bytes are and
            // not only in the slice. (M71)
            for guessed in [
                "cide_agents_config",
                "cide_agent_override",
                "cide_llm_provider",
                "cide_llm_pool",
            ] {
                run.send(json!({
                    "jsonrpc": "2.0", "id": 4, "method": "tools/call",
                    "params": { "name": guessed, "arguments": { "maxConcurrent": 9 } },
                }));
                let refused = run.recv();
                assert_eq!(
                    refused["error"]["code"],
                    json!(METHOD_NOT_FOUND),
                    "{guessed} answered a run"
                );
                assert!(refused["result"].is_null(), "{refused}");
            }
        }

        server.join().expect("the server thread");
        let _ = std::fs::remove_file(&path);
    }

    /// The `name`s out of a `tools/list` reply, in the order they were advertised.
    fn tool_names(reply: Value) -> Vec<&'static str> {
        reply["result"]["tools"]
            .as_array()
            .expect("an array")
            .iter()
            .filter_map(|entry| entry["name"].as_str())
            // Mapped back onto the constants so the assertions above compare against the one
            // definition of each name rather than against a literal typed twice.
            .map(|name| {
                tools::tool::EVERY
                    .iter()
                    .find(|known| **known == name)
                    .copied()
                    .unwrap_or_else(|| panic!("{name} is advertised and is not a known tool"))
            })
            .collect()
    }

    #[test]
    fn a_store_backed_sink_mutates_and_reports_that_it_did() {
        // The one test here that touches a real `TaskStore`, which needs no app: it is the piece
        // of glue between `cide_agents::tools` and `cide_tasks` that nothing else exercises.
        let root = std::env::temp_dir().join(format!("cide-agent-sink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");

        let sink = StoreSink {
            store: Arc::new(TaskStore::open(&root)),
            project: ProjectId::new(),
            author: TaskAuthor::Orchestrator,
            changed: AtomicBool::new(false),
            mutations: Mutex::new(Vec::new()),
            cwd: root.clone(),
        };

        assert!(sink.list().expect("list").is_empty());
        assert!(
            !sink.changed.load(Ordering::Relaxed),
            "a read is not a change"
        );
        assert!(
            sink.mutations.lock().is_empty(),
            "a read left a mutation for the trigger to act on"
        );

        let made = sink
            .create(
                "Add the retry bar",
                "why",
                Some(&AgentId("developer".into())),
                None,
                &[],
                &[],
                false,
            )
            .expect("create");
        assert!(sink.changed.load(Ordering::Relaxed));
        assert_eq!(made.status, TaskStatus::Todo);
        assert_eq!(made.agent, Some(AgentId("developer".into())));

        // The author is the connection's, never the payload's — a comment written over this
        // socket is signed by whoever the header resolved to.
        let commented = sink
            .edit(
                &made.id,
                TaskEdit::Comment {
                    text: "done".into(),
                },
            )
            .expect("comment");
        assert_eq!(
            commented.comments.last().expect("a comment").author,
            TaskAuthor::Orchestrator
        );
        assert_eq!(
            sink.get(&made.id).expect("get").expect("present").id,
            made.id
        );

        // The mutation ledger the trigger reads: one entry per write, the creation carrying its
        // body as fresh text and the comment carrying its line — signed with the connection's
        // author, which is what the trigger's gate refuses run-authored entries by.
        let mutations = sink.mutations.lock();
        assert_eq!(mutations.len(), 2);
        assert!(mutations[0].before.is_none());
        assert_eq!(mutations[0].fresh_text, ["why"]);
        assert_eq!(
            mutations[1].before.as_ref().map(|task| task.id.clone()),
            Some(made.id.clone())
        );
        assert_eq!(mutations[1].fresh_text, ["done"]);
        assert!(
            mutations
                .iter()
                .all(|m| m.author == TaskAuthor::Orchestrator)
        );
        drop(mutations);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A run attaches a file it wrote in its own worktree, by a path relative to *that*, and the
    /// bytes land under the project root — the whole reason `TaskSink::cwd` and `root` are two
    /// different answers. (M39)
    #[test]
    fn a_run_attaches_from_its_worktree_and_the_file_lands_under_the_root() {
        let root = std::env::temp_dir().join(format!(
            "cide-agent-attach-{}-{:?}",
            std::process::id(),
            thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let worktree = root.join(".cide/worktrees/developer");
        std::fs::create_dir_all(&worktree).expect("worktree");
        std::fs::write(
            worktree.join("shot.png"),
            b"\x89PNG\r\n\x1a\n0000000000000000",
        )
        .expect("the run's own file");

        let sink = StoreSink {
            store: Arc::new(TaskStore::open(&root)),
            project: ProjectId::new(),
            author: TaskAuthor::Agent {
                agent: AgentId("developer".into()),
                label: "Developer".into(),
            },
            changed: AtomicBool::new(false),
            mutations: Mutex::new(Vec::new()),
            cwd: worktree.clone(),
        };
        let made = sink
            .create("Draw the thing", "", None, None, &[], &[], false)
            .expect("create");

        // A creation whose file is refused leaves no task behind — the id is spent, the row is
        // not. Through the tool, as a model's call arrives.
        let refused = tools::dispatch(
            tools::tool::TASK_CREATE,
            &json!({ "title": "With a missing file", "attachments": ["nope.png"] }),
            &sink,
        )
        .expect("dispatched");
        assert!(refused.is_error);
        assert_eq!(
            sink.list().unwrap().len(),
            1,
            "a refused creation must not leave a task with no files"
        );

        // Through the tool, so the relative path is resolved exactly as a model's call is.
        let answer = tools::dispatch(
            tools::tool::TASK_COMMENT,
            &json!({ "id": made.id, "text": "here it is", "attachments": ["shot.png"] }),
            &sink,
        )
        .expect("dispatched");
        let text = answer.to_json()["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(!answer.is_error, "{text}");

        let task = sink.get(&made.id).expect("get").expect("present");
        let comment = task.comments.last().expect("the comment");
        assert_eq!(comment.attachments.len(), 1);
        let record = &comment.attachments[0];
        assert_eq!(record.name, "shot.png");
        assert_eq!(record.kind, cide_ipc::AttachmentKind::Image);
        assert_eq!(record.added_by, sink.author);
        let on_disk = root.join(record.relative_path(&task.id));
        assert!(on_disk.is_file(), "{}", on_disk.display());
        assert!(
            !on_disk.starts_with(&worktree),
            "the copy lives under the project root, not the run's checkout"
        );
        // The render names that absolute path, which is what the next `Read` needs.
        assert!(text.contains(&on_disk.display().to_string()), "{text}");
        // And the ledger carries the comment's prose, so a mention in it still triggers.
        let mutations = sink.mutations.lock();
        assert_eq!(
            mutations.last().expect("recorded").fresh_text,
            ["here it is"]
        );
        drop(mutations);

        // A path that is not there refuses with the *resolved* path in the sentence, and leaves
        // no comment behind.
        let refused = tools::dispatch(
            tools::tool::TASK_ATTACH,
            &json!({ "id": made.id, "paths": ["missing.png"] }),
            &sink,
        )
        .expect("dispatched");
        let text = refused.to_json()["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(refused.is_error);
        assert!(
            text.contains(&worktree.join("missing.png").display().to_string()),
            "{text}"
        );
        assert_eq!(sink.get(&made.id).unwrap().unwrap().attachments.len(), 0);

        let _ = std::fs::remove_dir_all(&root);
    }

    // ======================================================================================
    // The product-owner loop. (M18)
    // ======================================================================================

    fn turn(project: ProjectId, agent: &str, task: Option<&str>) -> Turn {
        ended_turn(project, agent, task, TurnOutcome::HandedBack)
    }

    // --------------------------------------------------------------------------------------
    // One reviewer per task. (M79)
    // --------------------------------------------------------------------------------------

    fn reviewer(project: ProjectId, task: &str, session: SessionId) -> Reviewer {
        Reviewer {
            project,
            task: TaskId(task.to_string()),
            session,
        }
    }

    #[test]
    fn a_task_with_an_open_reviewer_is_told_rather_than_given_a_second_one() {
        let project = ProjectId::new();
        let session = SessionId::new();
        let mut open = vec![reviewer(project, "t-12", session)];
        assert_eq!(
            live_reviewer(&mut open, project, &TaskId("t-12".into()), |_| true),
            Some(session),
            "the burst goes to the tab that already owns this task"
        );
    }

    #[test]
    fn another_task_in_the_same_project_gets_its_own_reviewer() {
        let project = ProjectId::new();
        let mut open = vec![reviewer(project, "t-12", SessionId::new())];
        assert_eq!(
            live_reviewer(&mut open, project, &TaskId("t-13".into()), |_| true),
            None
        );
    }

    #[test]
    fn the_same_task_id_in_another_project_is_a_different_task() {
        let mine = ProjectId::new();
        let mut open = vec![reviewer(ProjectId::new(), "t-12", SessionId::new())];
        assert_eq!(
            live_reviewer(&mut open, mine, &TaskId("t-12".into()), |_| true),
            None,
            "task ids are per project; a row from another one must not suppress this tab"
        );
    }

    #[test]
    fn a_closed_tab_stops_suppressing_its_tasks_next_reviewer() {
        let project = ProjectId::new();
        let mut open = vec![reviewer(project, "t-12", SessionId::new())];
        assert_eq!(
            live_reviewer(&mut open, project, &TaskId("t-12".into()), |_| false),
            None,
            "the session is gone, so the tab was closed and the task has no reviewer"
        );
        assert!(
            open.is_empty(),
            "and the row is pruned rather than re-tested for the life of the process"
        );
    }

    #[test]
    fn pruning_keeps_the_tabs_that_are_still_open() {
        let project = ProjectId::new();
        let live = SessionId::new();
        let dead = SessionId::new();
        let mut open = vec![
            reviewer(project, "t-1", dead),
            reviewer(project, "t-2", live),
        ];
        assert_eq!(
            live_reviewer(&mut open, project, &TaskId("t-2".into()), |session| session
                == live),
            Some(live)
        );
        assert_eq!(open.len(), 1, "only the closed one went");
        assert_eq!(open[0].session, live);
    }

    /// `agent` is the role's **label**; its id is derived the way a real definition's is, by
    /// lowercasing and replacing what the grammar refuses — so a fixture labelled `3D Artist`
    /// carries the id `3d-artist`, which is the pair that caught the branch bug.
    fn ended_turn(
        project: ProjectId,
        agent: &str,
        task: Option<&str>,
        outcome: TurnOutcome,
    ) -> Turn {
        let id: String = agent
            .chars()
            .map(|c| {
                let c = c.to_ascii_lowercase();
                if c.is_ascii_lowercase() || c.is_ascii_digit() {
                    c
                } else {
                    '-'
                }
            })
            .collect();
        Turn {
            project,
            route: NudgeRoute::Primary,
            agent: AgentId(id.trim_matches('-').to_string()),
            agent_label: agent.to_string(),
            task: task.map(|id| TaskId(id.to_string())),
            outcome,
        }
    }

    /// **Finding 8, on the last prompt path that inherits it.**
    ///
    /// A PTY write is keystrokes and the terminator is Enter, so an embedded newline is another
    /// Enter: a two-line nudge would submit its first half as a turn and feed the rest in as a
    /// second one, and the product owner would answer a fragment. Neither of the two strings this
    /// line interpolates is chosen by this process — an agent label comes out of a committed
    /// markdown file, a task title out of a tracker any agent may write — so both are asserted
    /// with the hostile shapes in them, `\r` as well as `\n`.
    /// **The review prompt is one line too, and it has more strings to be wrong about.**
    ///
    /// `the_nudge_is_one_line_whatever_the_task_is_called` makes this claim for the console
    /// road; this is the same claim for the tab road, and it matters at least as much. The tab's
    /// prompt interpolates an agent label *twice* — once as prose and once inside a branch name
    /// — plus a task id and a task title, none of which this process chooses. A newline in any
    /// of them is a second Enter that submits the first half of the reviewer's instructions as
    /// its own turn, leaving a fresh `claude` answering a fragment with no idea what it is for.
    #[test]
    fn the_review_prompt_is_one_line_whatever_anybody_called_anything() {
        let project = ProjectId::new();
        let hostile = "Fix the parser\nand then\r\nrewrite the lexer";

        for prompt in [
            review_prompt(
                DEFAULT_REVIEW_PROMPT,
                &[turn(project, "developer", Some("t-17"))],
                Some(hostile),
            ),
            review_prompt(
                DEFAULT_REVIEW_PROMPT,
                &[turn(project, "dev\neloper", Some("t-17"))],
                Some(hostile),
            ),
            review_prompt(
                DEFAULT_REVIEW_PROMPT,
                &[turn(project, "developer", None)],
                Some(hostile),
            ),
            review_prompt(DEFAULT_REVIEW_PROMPT, &[], None),
            review_prompt(
                DEFAULT_REVIEW_PROMPT,
                &[
                    ended_turn(project, "developer", Some("t-1"), TurnOutcome::Failed),
                    ended_turn(
                        project,
                        "qa",
                        Some("t-2"),
                        TurnOutcome::Finished { code: 0 },
                    ),
                ],
                Some(hostile),
            ),
        ] {
            assert!(!prompt.contains('\n'), "a newline survived: {prompt}");
            assert!(
                !prompt.contains('\r'),
                "a carriage return survived: {prompt}"
            );
        }
    }

    /// The prompt names the run's branch, because that is what turns "go and look at the work"
    /// into a `git diff` — and it must **not** invent one for a task-less run, which stands in
    /// the project root and has no branch at all (M40).
    #[test]
    fn the_review_prompt_names_the_branch_only_when_there_is_one() {
        let project = ProjectId::new();

        let with_task = review_prompt(
            DEFAULT_REVIEW_PROMPT,
            &[turn(project, "developer", Some("t-9"))],
            Some("Wire it"),
        );
        assert!(
            with_task.contains("cide/developer-t-9"),
            "the reviewer was not told where to look: {with_task}"
        );
        assert!(with_task.contains("t-9"));
        assert!(with_task.contains("Wire it"));

        let adhoc = review_prompt(
            DEFAULT_REVIEW_PROMPT,
            &[turn(project, "developer", None)],
            None,
        );
        assert!(
            !adhoc.contains("cide/"),
            "a branch was invented for a run that has none: {adhoc}"
        );
        assert!(adhoc.contains("no task"));
    }

    /// **The branch is the one `cide_agents::checkout_name` builds, not the label.**
    ///
    /// The reported bug, and the shape of it is worth keeping: a role *labelled* `3D Artist`
    /// has the id `3d-artist`, and the first cut interpolated the label — so the reviewer was
    /// told to diff `cide/3D Artist-t-1060`, a branch with a space and capitals in it that has
    /// never existed and that `cide_git::worktree`'s whitelist refuses by construction. The
    /// task id is slugged into it too, which a hand-written `format!` also skipped.
    #[test]
    fn the_branch_is_the_one_the_worktree_actually_builds() {
        let project = ProjectId::new();
        let prompt = review_prompt(
            DEFAULT_REVIEW_PROMPT,
            &[turn(project, "3D Artist", Some("T-1060"))],
            Some("Seamless sand"),
        );

        let expected = cide_agents::checkout_name(
            &AgentId("3d-artist".into()),
            Some(&TaskId("T-1060".into())),
        );
        assert_eq!(expected, "3d-artist-t-1060", "the composer itself moved");
        assert!(
            prompt.contains(&format!("cide/{expected}")),
            "the reviewer was sent to a branch that does not exist: {prompt}"
        );
        assert!(
            !prompt.contains("cide/3D Artist"),
            "the label leaked into a ref name: {prompt}"
        );
        // The label still names *who*, because that is what a person reads.
        assert!(prompt.contains("`3D Artist`"), "{prompt}");
    }

    /// **The reviewer is told to finish the task, not to have an opinion about it.**
    ///
    /// `finishInNewTab` means the console is *not* told, so a verdict written into a comment
    /// that nothing reads would end the loop exactly where M18 found it. The prompt therefore
    /// names both ways to put the task down — done, or back to the role that built it — and
    /// names the role by **id**, which is what `cide_agent_dispatch` answers to.
    #[test]
    fn the_reviewer_is_told_how_to_finish_or_hand_back() {
        let project = ProjectId::new();
        let prompt = review_prompt(
            DEFAULT_REVIEW_PROMPT,
            &[turn(project, "3D Artist", Some("t-1060"))],
            None,
        );

        for handle in [
            "mcp__cide__cide_task_get",
            "mcp__cide__cide_task_comment",
            "mcp__cide__cide_task_update",
            "mcp__cide__cide_agent_dispatch",
            // The one that was missing, and whose absence produced a reviewer reporting that
            // the branch "isn't merged into main yet; I left that to you".
            "mcp__cide__cide_agent_integrate",
        ] {
            assert!(prompt.contains(handle), "{handle} is not named: {prompt}");
        }
        // Both the dispatch and the merge have to be addressable: the id, not the label.
        assert!(prompt.contains("`3d-artist`"), "{prompt}");
        // Both outcomes, and the loop's one bound.
        assert!(prompt.contains("set the task to done"), "{prompt}");
        assert!(prompt.contains("hand it back"), "{prompt}");
        // The merge comes *before* done, or a task is closed over work that never landed.
        let merged = prompt.find("cide_agent_integrate").expect("the merge");
        let done = prompt.find("set the task to done").expect("the close");
        assert!(
            merged < done,
            "the task is closed before the branch is merged: {prompt}"
        );
        assert!(
            prompt.contains("already been sent back for the same reason"),
            "nothing stops a reviewer re-dispatching for ever: {prompt}"
        );
        // Out-of-scope findings go on the board, not into the verdict. Without this a reviewer
        // has two bad options for something it noticed in passing — widen the task it is about
        // to close, or write it into a comment that is read once and never again.
        assert!(
            prompt.contains("mcp__cide__cide_task_create"),
            "a reviewer has nowhere to put what it noticed in passing: {prompt}"
        );
        // **"Nothing to merge" is a normal answer, not a fault to report.** It is what
        // `Integrated::UpToDate` says, and on a real project it is common: a re-dispatched run
        // that added no commits leaves a branch already in `main`, usually merged by the review
        // before this one. Without this the reviewer reported it to the user as a problem —
        // "the branch isn't merged; I left that to you" wearing its opposite face.
        assert!(
            prompt.contains("nothing to merge"),
            "the reviewer will read an up-to-date branch as a failure: {prompt}"
        );
        // And it must not invite the reviewer to do the work itself — that is a second author
        // on a branch the role still holds a checkout of.
        assert!(prompt.contains("do not fix it yourself"), "{prompt}");
    }

    /// A burst says how many others ended with it, so the reviewer knows to look further —
    /// but a single turn must not be decorated with "0 other turns", which reads as a bug.
    #[test]
    fn a_lone_turn_mentions_no_others() {
        let project = ProjectId::new();
        let one = review_prompt(
            DEFAULT_REVIEW_PROMPT,
            &[turn(project, "developer", Some("t-1"))],
            None,
        );
        assert!(!one.contains("other subagent"), "{one}");

        let three = review_prompt(
            DEFAULT_REVIEW_PROMPT,
            &[
                turn(project, "a", Some("t-1")),
                turn(project, "b", Some("t-2")),
                turn(project, "c", Some("t-3")),
            ],
            None,
        );
        assert!(three.contains("2 other subagent turn(s)"), "{three}");
        assert!(three.contains("mcp__cide__cide_agent_runs"), "{three}");
    }

    /// The tab is named for the run, so five of them in a strip can be told apart — and the
    /// title is clipped, because a task id is cide's and a task title is anybody's.
    #[test]
    fn the_review_tab_is_named_for_the_run() {
        let project = ProjectId::new();
        assert_eq!(
            review_tab_title(&[turn(project, "developer", Some("t-12"))]),
            "Review: developer · t-12"
        );
        assert_eq!(
            review_tab_title(&[turn(project, "developer", None)]),
            "Review: developer"
        );
        // A label that is all whitespace is not a name; the tab still says what it is.
        assert_eq!(
            review_tab_title(&[turn(project, "  ", None)]),
            "Review: subagent"
        );
        assert_eq!(review_tab_title(&[]), "Review");

        let long = review_tab_title(&[turn(project, &"x".repeat(400), Some("t-1"))]);
        assert!(
            long.chars().count() <= TITLE_BUDGET + 1,
            "a role could set the tab width: {} chars",
            long.chars().count()
        );
    }

    #[test]
    fn the_nudge_is_one_line_whatever_the_task_is_called() {
        let project = ProjectId::new();
        let hostile = "Fix the parser\nand then\r\nrewrite the lexer";

        for line in [
            nudge_line(&[turn(project, "developer", Some("t-17"))], Some(hostile)),
            nudge_line(&[turn(project, "dev\neloper", Some("t-17"))], Some(hostile)),
            nudge_line(&[turn(project, "developer", None)], Some(hostile)),
            nudge_line(&[], None),
            nudge_line(
                &[
                    turn(project, "developer", Some("t-1")),
                    turn(project, "qa", Some("t-2")),
                ],
                Some(hostile),
            ),
        ] {
            assert!(!line.contains('\n'), "an embedded Enter in `{line}`");
            assert!(!line.contains('\r'), "an embedded Enter in `{line}`");
            assert!(!line.is_empty());
            // The tools, not a restatement of them: the roster paragraph in `cmd::session`
            // already taught this session the whole vocabulary at spawn.
            assert!(line.contains("mcp__cide__cide_agent_runs"), "{line}");
        }
    }

    /// The line says what actually moved, so the orchestrator has somewhere to go.
    #[test]
    fn the_nudge_names_the_role_and_the_task_that_moved() {
        let project = ProjectId::new();

        let one = nudge_line(
            &[turn(project, "developer", Some("t-17"))],
            Some("Fix the parser"),
        );
        assert!(one.contains("`developer`"), "{one}");
        assert!(one.contains("t-17"), "{one}");
        assert!(one.contains("Fix the parser"), "{one}");

        // No tracker open, so no title — the id is the part that matters and it is still there.
        let untitled = nudge_line(&[turn(project, "developer", Some("t-17"))], None);
        assert!(untitled.contains("t-17"), "{untitled}");

        // An ad-hoc run has no task, and the line must not send the reader to one.
        let adhoc = nudge_line(&[turn(project, "developer", None)], None);
        assert!(!adhoc.contains("task t-"), "{adhoc}");
        assert!(adhoc.contains("no task"), "{adhoc}");

        // A burst says how many, and names the last one so there is still a thread to pull.
        let many = nudge_line(
            &[
                turn(project, "developer", Some("t-1")),
                turn(project, "qa", Some("t-2")),
                turn(project, "artist", Some("t-3")),
            ],
            Some("Redraw the icons"),
        );
        assert!(many.starts_with("3 subagent turns"), "{many}");
        assert!(many.contains("`artist`"), "{many}");
        assert!(many.contains("t-3"), "{many}");

        // And a title nobody bounded does not become the prompt.
        let long = nudge_line(
            &[turn(project, "developer", Some("t-17"))],
            Some(&"x".repeat(4_000)),
        );
        assert!(long.chars().count() < 400, "an unbounded title: {long}");
        assert!(long.contains('…'), "{long}");
    }

    /// The line says *how* the turn ended, because the orchestrator's next move differs: a
    /// handed-back run is answered or retried, a finished or failed one has no child behind it.
    /// An opencode run finishes as its normal end, which is why outcomes could not stay silent.
    #[test]
    fn the_nudge_says_how_the_turn_ended() {
        let project = ProjectId::new();

        let handed = nudge_line(&[turn(project, "developer", Some("t-17"))], None);
        assert!(handed.contains("handed its turn back"), "{handed}");

        let finished = nudge_line(
            &[ended_turn(
                project,
                "developer",
                Some("t-17"),
                TurnOutcome::Finished { code: 0 },
            )],
            None,
        );
        assert!(finished.contains("finished (exit 0)"), "{finished}");

        let failed = nudge_line(
            &[ended_turn(project, "developer", None, TurnOutcome::Failed)],
            None,
        );
        assert!(failed.contains("failed"), "{failed}");

        // A burst renders the last turn's outcome, and a non-zero code is carried verbatim.
        let many = nudge_line(
            &[
                turn(project, "developer", Some("t-1")),
                ended_turn(
                    project,
                    "qa",
                    Some("t-2"),
                    TurnOutcome::Finished { code: 101 },
                ),
            ],
            None,
        );
        assert!(many.contains("(finished (exit 101))"), "{many}");
    }

    /// **A busy orchestrator's nudge is held and redelivered, not lost.**
    ///
    /// The drop this replaces was justified by "the next run to finish will nudge again" — false
    /// for the last run of a burst, whose ending the orchestrator then simply never heard about.
    /// The state table: a hold puts the turns back and unlatches; `rearm` latches exactly one
    /// flusher and answers `false` when there is nothing held (the every-session-event fast
    /// path) or when one is already waiting; the held turns flush once due; and a hold past the
    /// cap drops the *oldest*, keeping the "most recently" the line names true.
    #[test]
    fn a_held_nudge_is_redelivered_when_the_session_frees() {
        let coalescer = NudgeCoalescer::new();
        let project = ProjectId::new();

        assert!(
            !coalescer.rearm(),
            "nothing held, and a flusher was latched"
        );

        // A burst settles, the flusher takes it — and delivery finds the orchestrator busy.
        assert!(coalescer.mark(turn(project, "developer", Some("t-1"))));
        {
            let mut state = coalescer.state.lock();
            state.last = Some(Instant::now() - NUDGE_COALESCE - Duration::from_millis(10));
        }
        let due = coalescer.take_due().expect("settled");
        coalescer.hold(due);

        assert!(
            coalescer.take_due().is_none(),
            "held turns flushed with no flusher armed"
        );
        assert!(coalescer.rearm(), "the ready edge must start a flusher");
        assert!(!coalescer.rearm(), "and exactly one");

        {
            let mut state = coalescer.state.lock();
            state.last = Some(Instant::now() - NUDGE_COALESCE - Duration::from_millis(10));
        }
        let redelivered = coalescer.take_due().expect("due again after the re-arm");
        assert_eq!(redelivered.len(), 1);
        assert_eq!(redelivered[0].task, Some(TaskId("t-1".into())));

        // The cap drops the oldest: hold more than fits and the newest survive.
        let many: Vec<Turn> = (0..NUDGE_HOLD_CAP + 5)
            .map(|n| turn(project, "developer", Some(&format!("t-{n}"))))
            .collect();
        coalescer.hold(many);
        assert!(coalescer.rearm());
        {
            let mut state = coalescer.state.lock();
            state.last = Some(Instant::now() - NUDGE_COALESCE - Duration::from_millis(10));
        }
        let capped = coalescer.take_due().expect("due");
        assert_eq!(capped.len(), NUDGE_HOLD_CAP);
        assert_eq!(
            capped.last().and_then(|turn| turn.task.clone()),
            Some(TaskId(format!("t-{}", NUDGE_HOLD_CAP + 4))),
            "the cap must drop the oldest, never the most recent"
        );
    }

    /// **Six agents finishing together are one prompt.**
    ///
    /// The property the coalescer exists for, and the reason its window is measured in seconds
    /// rather than the emit's milliseconds: what is being started here is a *turn*, and six of
    /// them would be six answers to one fact. Nothing is due while the burst is still arriving,
    /// exactly one thread is asked to flush it, and the flusher slot frees again afterwards.
    #[test]
    fn a_burst_of_finished_turns_is_one_prompt() {
        let coalescer = NudgeCoalescer::new();
        let project = ProjectId::new();

        let flushers = (0..6)
            .filter(|n| coalescer.mark(turn(project, "developer", Some(&format!("t-{n}")))))
            .count();
        assert_eq!(flushers, 1, "a burst must not type a prompt per turn");
        assert!(
            coalescer.take_due().is_none(),
            "the nudge fired while the burst was still arriving"
        );

        {
            // Poked rather than slept through, the way `agents::a_run_that_never_stops_reporting`
            // does it: the alternative is a test that sleeps for two seconds.
            let mut state = coalescer.state.lock();
            state.last = Some(Instant::now() - NUDGE_COALESCE - Duration::from_millis(10));
        }
        let due = coalescer.take_due().expect("the burst has settled");
        assert_eq!(due.len(), 6, "six turns, one prompt");
        assert_eq!(
            nudge_line(&due, None).split("subagent").next(),
            Some("6 "),
            "the prompt did not say how many"
        );
        assert!(coalescer.take_due().is_none(), "and it fired twice");
        assert!(
            coalescer.mark(turn(project, "qa", None)),
            "the flusher slot never freed"
        );
    }

    /// The ceiling, which is what keeps a continuously-dispatching project from going silent.
    ///
    /// A bare trailing debounce never fires while turns keep ending inside its window, and a
    /// project with six roles working can do that for an hour.
    #[test]
    fn a_project_whose_runs_never_stop_finishing_is_still_told() {
        let coalescer = NudgeCoalescer::new();
        let project = ProjectId::new();
        coalescer.mark(turn(project, "developer", None));
        {
            let mut state = coalescer.state.lock();
            state.first = Some(Instant::now() - NUDGE_CEILING - Duration::from_millis(10));
            // Still arriving: the trailing debounce alone would say "not yet".
            state.last = Some(Instant::now());
        }
        assert_eq!(
            coalescer.take_due().map(|due| due.len()),
            Some(1),
            "the ceiling did not override the trailing debounce"
        );
    }

    /// A flusher that could not be started must not silence every later nudge.
    #[test]
    fn giving_up_on_a_burst_unlatches_the_next_one() {
        let coalescer = NudgeCoalescer::new();
        let project = ProjectId::new();
        assert!(coalescer.mark(turn(project, "developer", None)));
        coalescer.give_up();
        assert!(
            coalescer.mark(turn(project, "qa", None)),
            "one failed thread spawn ended the product-owner loop for the process"
        );
    }

    /// Two projects each get their own product owner told, and each is told once.
    #[test]
    fn a_burst_spanning_two_projects_is_one_prompt_each() {
        let (one, two) = (ProjectId::new(), ProjectId::new());
        let grouped = by_route(vec![
            turn(one, "developer", Some("t-1")),
            turn(two, "qa", Some("t-9")),
            turn(one, "artist", Some("t-2")),
        ]);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].0, (one, NudgeRoute::Primary), "first-seen order");
        assert_eq!(grouped[0].1.len(), 2);
        assert_eq!(grouped[1].0, (two, NudgeRoute::Primary));
        assert_eq!(grouped[1].1.len(), 1);
    }

    /// (M40) One project's burst can be owed to two panes — the console and the pane that
    /// dispatched — and each is typed once, about its own runs only. A line naming both would
    /// land in one of them about work the other is waiting on.
    #[test]
    fn a_burst_spanning_two_panes_of_one_project_is_one_prompt_each() {
        let project = ProjectId::new();
        let pane = SessionId::new();
        let to_pane = |agent: &str, task: &str| Turn {
            route: NudgeRoute::Session(pane),
            ..turn(project, agent, Some(task))
        };
        let grouped = by_route(vec![
            turn(project, "developer", Some("t-1")),
            to_pane("qa", "t-9"),
            turn(project, "artist", Some("t-2")),
            to_pane("qa", "t-10"),
        ]);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].0, (project, NudgeRoute::Primary));
        assert_eq!(grouped[0].1.len(), 2);
        assert_eq!(grouped[1].0, (project, NudgeRoute::Session(pane)));
        assert_eq!(grouped[1].1.len(), 2);
    }

    /// (M40) The pane that asked is the pane that hears — while it is still there. A route to a
    /// session that has exited, or that is no longer a Claude pane of this project, resolves to
    /// the primary rather than to nothing: the fact is not dropped for want of its first address.
    #[test]
    fn a_gone_target_pane_falls_back_to_the_primary() {
        let (mut ws, project, primary) = workspace_with_a_project();
        let pane = SessionId::new();
        split_into_console(&mut ws, project, PaneKind::Claude, pane);

        assert_eq!(
            nudge_target(&ws, project, &NudgeRoute::Primary, |_| true),
            Some(primary)
        );
        assert_eq!(
            nudge_target(&ws, project, &NudgeRoute::Session(pane), |_| true),
            Some(pane),
            "alive and still a Claude pane of the project: it hears"
        );
        assert_eq!(
            nudge_target(&ws, project, &NudgeRoute::Session(pane), |_| false),
            Some(primary),
            "its child has exited: the console hears instead"
        );
        let stranger = SessionId::new();
        assert_eq!(
            nudge_target(&ws, project, &NudgeRoute::Session(stranger), |_| true),
            Some(primary),
            "alive but not one of this project's panes: the console hears instead"
        );
        assert_eq!(
            nudge_target(&ws, ProjectId::new(), &NudgeRoute::Primary, |_| true),
            None,
            "a project that is gone has nobody to tell"
        );
    }

    /// **A busy orchestrator is not typed into.**
    ///
    /// Bytes written into a session mid-turn land in whatever the CLI is composing. `Busy` and
    /// `AwaitingPermission` now *hold* the nudge for redelivery (the test above); every other
    /// refused state still loses it, and the unknown case matters as much as the known ones:
    /// `HookServer::state` answers `Spawning` for a session it has never seen — which is every
    /// session in a build whose hook socket failed to bind — and losing the nudge is the right
    /// way to be wrong about that.
    #[test]
    fn only_an_idle_or_awaiting_orchestrator_may_be_typed_into() {
        for state in [SessionState::Idle, SessionState::AwaitingInput] {
            assert!(may_be_typed_into(state), "{state:?}");
        }
        for state in [
            SessionState::Spawning,
            SessionState::Splash,
            SessionState::Busy,
            SessionState::AwaitingPermission,
            SessionState::Paused,
            SessionState::Exited { code: 0 },
        ] {
            assert!(!may_be_typed_into(state), "typed into a {state:?} session");
        }
    }

    /// **The setting is read from the disk every time, and never cached.**
    ///
    /// `.cide/config.json` is committed, so a `git checkout` — or the user's own editor — can
    /// switch this off under a running app. Asserted by moving the file *between* two calls, which
    /// is the only shape that can tell "read fresh" from "read once and remembered".
    #[test]
    fn the_setting_is_honoured_and_re_read_on_every_nudge() {
        let root = std::env::temp_dir().join(format!("cide-nudge-setting-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".cide")).expect("temp dir");
        let config = root.join(".cide").join("config.json");

        // A project that has never heard of the key. On, because the loop it closes is the
        // feature — see `AgentsConfig::nudge_orchestrator`.
        assert!(
            cide_agents::config::load(&root).agents.nudge_orchestrator,
            "the default is the way out, not the way in"
        );

        std::fs::write(&config, r#"{ "agents": { "nudgeOrchestrator": false } }"#).expect("write");
        assert!(
            !cide_agents::config::load(&root).agents.nudge_orchestrator,
            "a project that said no was typed at"
        );

        std::fs::write(&config, r#"{ "agents": { "nudgeOrchestrator": true } }"#).expect("write");
        assert!(
            cide_agents::config::load(&root).agents.nudge_orchestrator,
            "the answer was cached across a checkout"
        );

        // And a file nothing can parse does not silently take the feature away either.
        std::fs::write(&config, "{{{ not json").expect("write");
        assert!(cide_agents::config::load(&root).agents.nudge_orchestrator);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **The bytes, into a real child, terminated by a real Enter.**
    ///
    /// Everything above is a string. This is the write, against a `/bin/sh` on a real PTY —
    /// `agents.rs`'s own tests fork one for the same reason, that what is under test is the
    /// kernel's behaviour and the child's identity is irrelevant to it. The shell's `read`
    /// returning at all is the assertion that matters: a line arrives only because the `\r` was
    /// translated to a newline by the terminal discipline, which is precisely the claim finding 8
    /// makes about why a prompt is submitted this way and why it may not contain another one.
    ///
    /// What a shell's `read` **cannot** see is a raw-mode TUI's paste detection — claude 2.1.245
    /// eats the trailing CR off exactly these bytes when they arrive as one chunk, which is why
    /// delivery goes through `crate::agents::type_submitted_line` and this test stays a test of
    /// the byte shape, not of the delivery.
    #[test]
    fn the_nudge_reaches_a_real_child_as_one_submitted_line() {
        let bytes = submit(&nudge_line(
            &[turn(ProjectId::new(), "developer", Some("t-17"))],
            Some("Fix the parser"),
        ))
        .expect("claude is a harness this build has");

        assert_eq!(
            bytes.iter().filter(|byte| **byte == b'\r').count(),
            1,
            "more than one Enter in one prompt"
        );
        assert_eq!(bytes.last(), Some(&b'\r'), "the Enter is not at the end");
        assert!(!bytes.contains(&b'\n'), "a newline is a second Enter");

        // `read` blocks until a line arrives; the `case` is what proves it is *this* line. Exit 3
        // rather than 1 so a shell that failed for its own reasons is distinguishable.
        let pty = cide_pty::PtySession::spawn(
            cide_pty::SpawnSpec::new("/bin/sh", std::env::temp_dir())
                .arg("-c")
                .arg("read line; case \"$line\" in *cide_agent_runs*) exit 0;; *) exit 3;; esac"),
        )
        .expect("spawn sh");

        // The child has to reach its `read` before the write, or the bytes land in a terminal
        // nothing is draining yet. They would still be buffered and still arrive — this is only
        // to keep the failure, if there is one, about the write rather than about the race.
        thread::sleep(Duration::from_millis(200));
        pty.write(bytes);

        let deadline = Instant::now() + Duration::from_secs(10);
        while pty.exit_status().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            pty.exit_status().map(|exit| exit.code),
            Some(0),
            "the nudge never arrived as a submitted line"
        );
    }
    // ======================================================================================
    // The epitaph (M67). Pure over `DeathFacts`, so the sentences are asserted here rather
    // than read out of a live app by somebody who had to stop a real agent to see one.
    // ======================================================================================

    fn facts(code: Option<i32>, stop: Option<StopHow>, reason: Option<&str>) -> DeathFacts {
        DeathFacts {
            task: cide_ipc::TaskId::from("t-4".to_string()),
            agent_label: "Developer".to_string(),
            code,
            stop: stop.map(|how| crate::agents::StopRecord {
                by: StopBy::Orchestrator,
                reason: reason.map(str::to_string),
                how,
                grace_secs: 60,
            }),
        }
    }

    /// **The two sentences that do not move.** This is the disambiguation.
    ///
    /// They are byte-identical to what they were before M67, and they are what the *unstopped*
    /// arms still produce — so an old board and a new one say the same thing with the same
    /// words. The point is not that they changed; it is that they now only fire when nobody
    /// asked. Editing them to mention a stop would re-merge the crash and the stop from the
    /// other direction, which is the bug this milestone exists to fix.
    #[test]
    fn a_death_nobody_ordered_reads_exactly_as_it_always_did() {
        let run = RunId::new();
        assert_eq!(
            epitaph(run, &facts(Some(129), None, None)),
            format!("run {run} (Developer) ended with exit 129 before finishing this task")
        );
        assert_eq!(
            epitaph(run, &facts(None, None, None)),
            format!("run {run} (Developer) failed before finishing this task")
        );
    }

    /// Every deliberate ending says so, says whose hand it was, and says which kind it was.
    #[test]
    fn a_deliberate_ending_is_distinguishable_from_a_crash_and_from_the_other_kinds() {
        let run = RunId::new();
        const WHY: &str = "the migration is going the wrong way";
        let why = Some(WHY);

        let wound = epitaph(run, &facts(Some(0), Some(StopHow::WoundDown), why));
        assert!(wound.contains("was stopped by the orchestrator"), "{wound}");
        assert!(wound.contains("wound down as asked"), "{wound}");
        assert!(wound.contains("its own last comment"), "{wound}");
        assert!(wound.contains(WHY), "{wound}");

        let forced = epitaph(
            run,
            &facts(
                Some(129),
                Some(StopHow::Forced {
                    why: "it did not wind down in time",
                }),
                why,
            ),
        );
        assert!(forced.contains("did not wind down in time"), "{forced}");
        assert!(forced.contains("exit 129"), "{forced}");

        // **"We asked and it would not go" and "we never asked" must not collapse.** That is
        // this milestone's own bug, one level down, and exactly what `exit 129` meaning both a
        // stop and a crash was.
        let outright = epitaph(
            run,
            &facts(Some(129), Some(StopHow::Immediate { why: None }), why),
        );
        assert!(outright.contains("killed outright"), "{outright}");
        assert!(outright.contains("without being asked"), "{outright}");
        assert_ne!(outright, forced);

        // And an unasked kill that was not the caller's choice names the obstacle. A stop that
        // silently declined to ask first is a feature that appears not to work.
        let blocked = epitaph(
            run,
            &facts(
                Some(129),
                Some(StopHow::Immediate {
                    why: Some("it was waiting on a permission prompt"),
                }),
                None,
            ),
        );
        assert!(
            blocked.contains("not asked to wind down first"),
            "{blocked}"
        );
        assert!(blocked.contains("permission prompt"), "{blocked}");

        let cancelled = epitaph(run, &facts(None, Some(StopHow::BeforeStart), why));
        assert!(cancelled.contains("before it started"), "{cancelled}");
        assert!(cancelled.contains("nothing ran"), "{cancelled}");

        let discarded = epitaph(run, &facts(None, Some(StopHow::Discarded), None));
        assert!(discarded.contains("without being resumed"), "{discarded}");
        assert!(discarded.contains("still on disk"), "{discarded}");

        // The reclaim: not a stop anybody asked for. Until M67 it borrowed the crash sentence
        // and reported a run whose turn had *already ended* as having died before finishing.
        let reclaimed = epitaph(run, &facts(Some(129), Some(StopHow::Reclaimed), None));
        assert!(
            reclaimed.contains("give its worktree to another task"),
            "{reclaimed}"
        );
        assert!(reclaimed.contains("nothing was interrupted"), "{reclaimed}");
        assert!(
            !reclaimed.contains("before finishing this task"),
            "the crash sentence must not leak here: {reclaimed}"
        );

        // Every stopped arm is distinguishable from the crash line, which is the whole ask.
        let crashed = epitaph(run, &facts(Some(129), None, None));
        for other in [
            &wound, &forced, &outright, &blocked, &cancelled, &discarded, &reclaimed,
        ] {
            assert_ne!(other, &crashed);
        }
    }

    /// No reason given: the clause comes off. `(none given)` belongs in a log, not on a board.
    #[test]
    fn an_epitaph_with_no_reason_carries_no_dangling_clause() {
        let run = RunId::new();
        for reason in [None, Some(""), Some("   ")] {
            let text = epitaph(run, &facts(Some(0), Some(StopHow::WoundDown), reason));
            assert!(!text.contains('—'), "{text}");
            assert!(!text.ends_with(' '), "{text}");
        }
    }
}
