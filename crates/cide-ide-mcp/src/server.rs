//! The WebSocket server a `claude` child connects to, and everything that routes between it
//! and the application.
//!
//! # Shape
//!
//! One server per project on `127.0.0.1:0`, one [`crate::lockfile::Lockfile`] naming the port
//! it landed on, one task per connection. The application takes [`IdeServer::events`] once and
//! acts on what comes out; nothing here touches a window, a pane or a file.
//!
//! # Why every notification is addressed and none are broadcast
//!
//! A project with two Claude panes is the normal case here, not an edge case. The CLI
//! announces itself with `ide_connected{pid}` and `cide-pty` already knows each pane's child
//! pid, so a connection resolves to a pane through two maps: connection to pid, and pid to
//! pane. Broadcasting a `selection_changed` instead would put the text the user highlighted
//! into the other pane's prompt, silently, with nothing on screen to explain it.
//!
//! That paragraph was written first, contradicted for a while, and is true again. A broadcast
//! `selection_changed_all` was added when the addressed one turned out to have no producer,
//! on the argument that a selection is "a fact about the editor" rather than a message aimed
//! at one conversation, so every `claude` in the project should hear it. From a chair it was
//! exactly the failure the paragraph above predicts: every caret move in any editor put
//! `⧉ Selected N lines from <file>` into the prompt of every `claude` in the project, and
//! the CLI sends that selection as context with the next message — so a conversation about
//! the Docker panel was handed the git panel's code because the user had glanced at it. The
//! broadcast is gone. The only selection a CLI hears is the one *Send lines to Claude*
//! addresses to it, in the same breath as the mention, and an editor that is merely being
//! read tells nobody anything.
//!
//! The pid map is filled by [`IdeServer::bind_pane`] rather than discovered here, because
//! only the application knows which `PtySession` owns a pid. Either order works: the two maps
//! are independent, and a notification sent before both halves exist is dropped with a log
//! line rather than guessed at.
//!
//! # Why the accept loop never waits for a human
//!
//! `openDiff` blocks the agent's turn, and the CLI sends `close_tab` down the **same
//! connection** that is waiting on the diff — from its own `beforeExit` and from its abort
//! handler. Handling a request inline would therefore deadlock the one socket that could
//! deliver the answer. So each connection is a task, and each request within it is a task
//! again; the connection loop only shuttles bytes.

use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{Value, json};
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as HandshakeRequest, Response as HandshakeResponse,
};
use tokio_tungstenite::tungstenite::http::{HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::protocol::Message;

use crate::diff_broker::{CancelReason, DiffBroker, DiffRequest};
use crate::lockfile::{self, Lockfile};
use crate::protocol::{
    self, AtMentioned, INVALID_PARAMS, Incoming, METHOD_NOT_FOUND, Notification, Response,
    SelectionChanged,
};
use crate::tools;
use crate::{IdeError, Result};

/// How many events may queue before a connection task waits for the application to catch up.
///
/// Bounded on purpose. An unbounded queue would let a stalled frontend accumulate diff
/// requests without limit; back-pressure at least keeps the number of un-shown diffs equal to
/// the number of agents that are actually blocked.
const EVENT_CAPACITY: usize = 64;

/// Answered when a client asks for `initialize` without naming a version.
///
/// The SDK inside 2.1.226 accepts exactly `2025-11-25`, `2025-06-18`, `2025-03-26`,
/// `2024-11-05` and `2024-10-07` back from a server and throws on anything else, so a version
/// invented here would end the handshake. Echoing the client's own choice is the only answer
/// that cannot be wrong; this is the fallback for a client that omits it, and it is in that
/// set.
const DEFAULT_PROTOCOL_VERSION: &str = "2025-06-18";

/// How long [`IdeServer::shutdown`] waits for connection tasks to finish writing.
///
/// Bounded because the wait is on a socket: a `claude` that has stopped reading would
/// otherwise keep the application from quitting for as long as it stayed alive. Two seconds is
/// far more than the flush of a few hundred bytes needs and far less than a user notices.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// Something the application has to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    /// A `claude` child connected and named its pid. The application answers this with
    /// [`IdeServer::bind_pane`] once it has matched the pid to a `PtySession`.
    Connected {
        connection: u64,
        pid: u32,
        /// What the CLI called itself in `initialize`: `clientInfo.version`.
        ///
        /// # The only version cide has that names the binary that actually connected
        ///
        /// `cide_claude::version::check_once` runs `claude --version` and latches the answer
        /// for the life of the process — deliberately, because re-probing would mean
        /// re-warning — so it describes whichever binary was on `PATH` at the first Claude
        /// spawn. The CLI self-updates underneath a running app. This names the build that
        /// read our lockfile, chose the WebSocket transport, presented the auth header and
        /// accepted our `initialize` reply, at the moment it did so.
        ///
        /// It was already on the wire and already parsed — `initialize_result` read
        /// `protocolVersion` out of the same `params` and discarded the rest — and recording
        /// it costs one field. `cide_app::ide` writes it to `cide_core::handshake`, and the
        /// Settings screen shows it.
        ///
        /// `Option`, because a client that omits `clientInfo` is a client we still serve. A
        /// missing version is "we do not know", which is a different answer from a version
        /// outside the range and must not be reported as one.
        client_version: Option<String>,
    },
    Disconnected {
        connection: u64,
    },
    /// A diff is waiting for a human. The application opens a tab for it, and answers through
    /// [`DiffBroker::resolve`].
    DiffRequested(DiffRequest),
    /// The CLI withdrew a diff. The pending request is already resolved by the time this
    /// arrives; what is left is a tab on screen that should go away.
    DiffWithdrawn {
        tab_name: String,
    },
    /// The CLI asked to reveal a file. Line numbers are 1-based here — the conversion from
    /// the wire's 0-based numbering happens in [`crate::tools::open_file`].
    OpenFile {
        path: String,
        start_line: Option<u32>,
        end_line: Option<u32>,
    },
}

/// One live WebSocket.
struct Conn {
    /// Anything written here is framed and sent by the connection's own loop. Holding a clone
    /// keeps the connection open, which is how in-flight tool results get onto the wire
    /// before a shutdown closes the socket.
    out: mpsc::UnboundedSender<Message>,
    /// From `ide_connected`. `None` until the CLI sends it, which is a window of a few
    /// milliseconds in practice but is not zero.
    pid: Option<u32>,
    /// From `initialize`'s `clientInfo.version`. See [`ServerEvent::Connected`].
    ///
    /// Recorded on the connection rather than passed straight through, because the two frames
    /// are separate: `initialize` is a request and `ide_connected` is a notification that
    /// arrives afterwards. In the real capture the order is `initialize`, then
    /// `notifications/initialized`, then `ide_connected` — so by the time there is a pid to
    /// announce, this is already set. If a future CLI reversed them the field would simply be
    /// `None` and the record would say "unknown", which is the honest degradation.
    client_version: Option<String>,
}

#[derive(Default)]
struct Connections {
    next_id: u64,
    open: HashMap<u64, Conn>,
    /// Filled by [`IdeServer::bind_pane`]. Keyed by pid rather than by connection because the
    /// application learns the pid when it spawns the child, long before that child connects.
    pane_of_pid: HashMap<u32, String>,
    /// Set by [`IdeServer::shutdown`] before it empties `open`, and never cleared.
    ///
    /// Aborting the accept loop is not enough on its own: a socket accepted a moment earlier
    /// may still be inside its WebSocket handshake, and registering it afterwards would put
    /// the only sender for a channel nobody will ever close again into a task that then never
    /// ends — holding the port and leaving that `claude` believing in an IDE that has gone.
    closed: bool,
}

struct Inner {
    broker: DiffBroker,
    /// Behind a lock because [`IdeServer::events`] may replace it; see that method.
    events: Mutex<mpsc::Sender<ServerEvent>>,
    conns: Mutex<Connections>,
    auth_token: String,
    /// Cloned by each connection task and dropped when that task has flushed its socket.
    ///
    /// [`IdeServer::shutdown`] drops the original and waits for the clones to go. Without that
    /// wait, `shutdown` returns while the `DIFF_REJECTED` frames it just produced are still
    /// queued, and the caller's next act is to signal the `claude` children — which is the
    /// difference between an agent turn that ends with a decision and one that ends with a
    /// transport error.
    alive: Mutex<Option<mpsc::UnboundedSender<()>>>,
    /// Where `getDiagnostics` reads from, once the app installs one.
    ///
    /// Behind the same kind of lock as [`Self::events`] and set after `start` for the same
    /// reason: the diagnostics store does not exist until the project's servers do. A server
    /// that was never given one answers `[]`, which is the true sentence it answered before
    /// this field existed.
    diagnostics: Mutex<Option<Arc<dyn tools::DiagnosticSource>>>,
}

impl Inner {
    async fn emit(&self, event: ServerEvent) -> bool {
        // Cloned out from under the lock: a `parking_lot` guard held across an `await` would
        // not compile, and would deadlock if it did.
        let tx = self.events.lock().clone();
        tx.send(event).await.is_ok()
    }

    /// `None` once the server has shut down; see [`Connections::closed`].
    fn register(&self, out: mpsc::UnboundedSender<Message>) -> Option<u64> {
        let mut c = self.conns.lock();
        if c.closed {
            return None;
        }
        c.next_id += 1;
        let id = c.next_id;
        c.open.insert(
            id,
            Conn {
                out,
                pid: None,
                client_version: None,
            },
        );
        Some(id)
    }

    fn unregister(&self, connection: u64) {
        self.conns.lock().open.remove(&connection);
    }

    fn set_pid(&self, connection: u64, pid: u32) {
        if let Some(conn) = self.conns.lock().open.get_mut(&connection) {
            conn.pid = Some(pid);
        }
    }

    /// Record what the client called itself in `initialize`.
    ///
    /// A blank or absent version is stored as `None` rather than as `Some("")`: an empty
    /// string would travel all the way to the Settings screen and be drawn as a version.
    fn set_client_version(&self, connection: u64, version: Option<&str>) {
        let version = version.map(str::trim).filter(|v| !v.is_empty());
        if let Some(conn) = self.conns.lock().open.get_mut(&connection) {
            conn.client_version = version.map(str::to_string);
        }
    }

    fn client_version(&self, connection: u64) -> Option<String> {
        self.conns
            .lock()
            .open
            .get(&connection)?
            .client_version
            .clone()
    }

    fn pane_for(&self, connection: u64) -> Option<String> {
        let c = self.conns.lock();
        let pid = c.open.get(&connection)?.pid?;
        c.pane_of_pid.get(&pid).cloned()
    }

    fn sender_for(&self, connection: u64) -> Option<mpsc::UnboundedSender<Message>> {
        self.conns
            .lock()
            .open
            .get(&connection)
            .map(|c| c.out.clone())
    }

    fn sender_for_pane(&self, pane: &str) -> Option<mpsc::UnboundedSender<Message>> {
        let c = self.conns.lock();
        c.open
            .values()
            .filter_map(|conn| Some((conn.pid?, conn)))
            .find(|(pid, _)| c.pane_of_pid.get(pid).map(String::as_str) == Some(pane))
            .map(|(_, conn)| conn.out.clone())
    }

    /// Exactly the panes [`Self::sender_for_pane`] would find a sender for.
    ///
    /// Deliberately the *same* join rather than a second reading of the same two maps: this
    /// answers "would an addressed notification land", and a predicate that agreed with
    /// `sender_for_pane` only most of the time would hand a caller a pane to route to that
    /// then dropped the message — which is the silent no-op the `Delivery` enum was added to
    /// end. Written as a filter over `sender_for_pane`'s own condition for that reason.
    ///
    /// Deduplicated, because `pane_of_pid` can hold two pids for one pane: a Claude pane that
    /// respawned is bound again under the new pid and the old entry only goes when that child
    /// is reaped. Two identical entries would make a candidate look twice as reachable to a
    /// caller ranking them, which is a distinction it must not be able to draw.
    fn addressable_panes(&self) -> Vec<String> {
        let c = self.conns.lock();
        let mut panes: Vec<String> = c
            .open
            .values()
            .filter_map(|conn| c.pane_of_pid.get(&conn.pid?).cloned())
            .collect();
        panes.sort();
        panes.dedup();
        panes
    }

    /// The pids of connected CLIs that no pane claims.
    ///
    /// The exact complement of [`Self::addressable_panes`] on the same two maps: a connection
    /// that got through the handshake and announced a pid which is not a key of `pane_of_pid`.
    /// Empty is the healthy state and the ordinary one.
    ///
    /// # Why this crate answers the question and does not act on it
    ///
    /// An unbound connection is not an error here. It is a fact — *somebody is on this
    /// project's server and nothing says which pane they are in* — and the only way to resolve
    /// it is to ask the operating system whose child that pid is, which needs a notion of
    /// panes, of the children cide forked, and of a process tree. None of the three belongs in
    /// a crate whose whole discipline is that it knows about sockets and nothing else. So the
    /// pids go out, `cide_app::ide` walks each one's ancestry against the pids it forked, and
    /// what comes back is an ordinary [`IdeServer::bind_pane`] call.
    ///
    /// Sorted and deduplicated for the same reason [`Self::addressable_panes`] is: the caller
    /// does one lookup per entry, and two connections from one pid would ask twice.
    fn unbound_pids(&self) -> Vec<u32> {
        let c = self.conns.lock();
        let mut pids: Vec<u32> = c
            .open
            .values()
            .filter_map(|conn| conn.pid)
            .filter(|pid| !c.pane_of_pid.contains_key(pid))
            .collect();
        pids.sort_unstable();
        pids.dedup();
        pids
    }

    /// How many CLIs are attached to this project's server at all.
    ///
    /// The number is what separates "no Claude is connected to cide" from "a Claude is
    /// connected, but not the one in that pane" — two different sentences for a user, and the
    /// only thing that distinguishes a missing feature from a mis-aimed one.
    fn connection_count(&self) -> usize {
        self.conns.lock().open.len()
    }
}

/// What became of an addressed notification.
///
/// Returned rather than only logged. "Nothing was listening" is by far the most common reason
/// an `@`-mention appears to do nothing, and the layer that can put that on screen is the one
/// that made the gesture — a `tracing::debug!` reaches a log file nobody has open, which from
/// the user's chair is indistinguishable from a control wired to nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Written to that pane's socket.
    Sent,
    /// No connection is bound to that pane.
    NoConnection {
        /// Every CLI attached to this project's server, in any pane — *including* one in the
        /// asked-for pane whose pid was never bound, which is the likeliest reason this
        /// variant is being returned at all. Zero means no `claude` anywhere in this project
        /// has completed the IDE handshake.
        ///
        /// Deliberately not "other connections": distinguishing them would need the count of
        /// connections whose pid *is* in `pane_of_pid`, and an unbound connection is exactly
        /// the one this number exists to account for. Callers that turn it into prose must
        /// not claim these are elsewhere. See `cmd::file::not_connected`.
        connections: usize,
    },
}

// No `is_sent` helper. One was written and deleted unused: every caller matches the enum
// exhaustively because the `NoConnection` count is the whole point of returning it, and a
// boolean accessor invites the one call site that throws that count away.

/// The IDE server for one project.
pub struct IdeServer {
    port: u16,
    inner: Arc<Inner>,
    accept: JoinHandle<()>,
    events_rx: Mutex<Option<mpsc::Receiver<ServerEvent>>>,
    /// Yields `None` once every connection task has finished. See [`Inner::alive`].
    drained: Mutex<Option<mpsc::UnboundedReceiver<()>>>,
    /// Removed from `~/.claude/ide/` when this drops. `None` when the server was started
    /// without publishing itself, which is what the tests do.
    lockfile: Option<Lockfile>,
}

impl IdeServer {
    /// Bind a loopback port, publish a lockfile naming it, and start accepting.
    pub async fn start(workspace_folders: Vec<PathBuf>) -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|e| IdeError::Bind(e.to_string()))?;

        // Bind before publishing: the lockfile's name *is* the port, so there is nothing to
        // announce until the kernel has assigned one.
        let port = listener
            .local_addr()
            .map_err(|e| IdeError::Bind(e.to_string()))?
            .port();
        let lock = Lockfile::publish(port, workspace_folders)?;

        let mut server = Self::attach(listener, lock.token().to_string())?;
        server.lockfile = Some(lock);
        Ok(server)
    }

    /// Start on an already-bound listener with a caller-supplied token, publishing nothing.
    ///
    /// What the tests use. [`start`](Self::start) writes into the user's real
    /// `~/.claude/ide`, which is shared with whatever editors they are actually running and
    /// is no place for a test suite to leave files.
    pub(crate) fn attach(listener: TcpListener, auth_token: String) -> Result<Self> {
        let port = listener
            .local_addr()
            .map_err(|e| IdeError::Bind(e.to_string()))?
            .port();

        let (events_tx, events_rx) = mpsc::channel(EVENT_CAPACITY);
        let (alive_tx, drained_rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            broker: DiffBroker::new(),
            events: Mutex::new(events_tx),
            conns: Mutex::new(Connections::default()),
            auth_token,
            alive: Mutex::new(Some(alive_tx)),
            diagnostics: Mutex::new(None),
        });

        let accept = tokio::spawn(accept_loop(listener, Arc::clone(&inner)));

        Ok(Self {
            port,
            inner,
            accept,
            events_rx: Mutex::new(Some(events_rx)),
            drained: Mutex::new(Some(drained_rx)),
            lockfile: None,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn broker(&self) -> DiffBroker {
        self.inner.broker.clone()
    }

    /// Take the event stream.
    ///
    /// A stream has one consumer, so the first caller receives the receiver made at startup.
    /// A second caller gets a fresh channel and the first one stops receiving, which is the
    /// only handover that does not require this method to panic or to return an `Option` for
    /// a case the application never reaches: it takes the stream once and keeps it.
    pub fn events(&self) -> mpsc::Receiver<ServerEvent> {
        if let Some(rx) = self.events_rx.lock().take() {
            return rx;
        }
        let (tx, rx) = mpsc::channel(EVENT_CAPACITY);
        *self.inner.events.lock() = tx;
        rx
    }

    /// Give this server something for `getDiagnostics` to read.
    ///
    /// A setter rather than a parameter to [`start`](Self::start), for the same reason
    /// [`events`](Self::events) is taken rather than handed in: the diagnostics store does not
    /// exist until the project's servers do. A server that has never been given one answers
    /// `[]` — the same true sentence it answered before this existed, which is also what keeps
    /// every test that calls `attach` compiling unchanged.
    pub fn set_diagnostics(&self, source: Arc<dyn tools::DiagnosticSource>) {
        *self.inner.diagnostics.lock() = Some(source);
    }

    /// Record which pane a child pid belongs to, so notifications can be addressed.
    pub fn bind_pane(&self, pid: u32, pane: String) {
        self.inner.conns.lock().pane_of_pid.insert(pid, pane);
    }

    /// Forget a pid's pane, so that a recycled pid cannot inherit the dead pane's selections.
    ///
    /// # This had no caller at all until now, and that is worth recording
    ///
    /// It was written with the doc comment above and wired to nothing, so `pane_of_pid` grew
    /// monotonically for the life of the server and every binding it ever made outlived the
    /// process it described. The hazard it names is real but narrow — a pid has to be recycled
    /// *and* the new process has to complete this project's token-gated handshake — which is
    /// exactly why it survived: nothing it protects against is visible until it happens.
    ///
    /// It matters more now than when it was written, because [`Self::addressable_panes`] turns
    /// this map into *routing*. A stale entry cannot resurrect a dead pane on its own (the
    /// dead `claude`'s connection is gone, so the join finds nothing), but a recycled pid
    /// would make an unrelated child answer for a pane that no longer holds it, and the
    /// fallback would then deliver a user's selection into it.
    ///
    /// Called from `cide-app`'s `report_exit` — the moment the child is reaped, which is the
    /// earliest instant at which its pid can be handed out again.
    pub fn unbind_pane(&self, pid: u32) {
        self.inner.conns.lock().pane_of_pid.remove(&pid);
    }

    /// The panes an addressed notification can actually reach right now.
    ///
    /// Opaque strings, exactly as [`Self::bind_pane`] takes them: this crate has no notion of
    /// tabs, projects or windows and must not gain one to answer a routing question. The
    /// caller ranks its own panes and asks this which of them are live — the ordering is a
    /// workspace decision and belongs where the workspace is.
    pub fn addressable_panes(&self) -> Vec<String> {
        self.inner.addressable_panes()
    }

    /// Connected CLIs that no pane claims, so the caller can work out who they are.
    ///
    /// See [`Inner::unbound_pids`] for why the answer is pids rather than an action. The
    /// caller is `cide_app::ide`, which walks each pid's ancestry and calls
    /// [`Self::bind_pane`] with what it finds.
    pub fn unbound_pids(&self) -> Vec<u32> {
        self.inner.unbound_pids()
    }

    /// Tell the `claude` in `pane` what the user selected — the text, not only the range.
    ///
    /// Addressed, exactly as [`Self::at_mentioned`] is, and for the same reason: the CLI puts
    /// the selection into its prompt as `⧉ Selected N lines` and sends the text as context
    /// with the next message, so this is *typing into a conversation*, whatever the method's
    /// name suggests. There is no broadcast form any more — see the module doc — so the one
    /// caller is *Send lines to Claude*, which sends this and then the mention to the same
    /// pane it resolved for both.
    ///
    /// The binding this needs is the same one the mention needs. A `claude` typed into a
    /// shell pane, or one behind a launcher, announces a pid cide never forked; that used to
    /// be the argument for a broadcast that needed no binding, and it is answered instead by
    /// `cide_app::ide::resolve_unbound_connections`, which walks the pid's ancestry to the
    /// pane that owns it before either notification is sent.
    pub fn selection_changed(&self, pane: &str, payload: SelectionChanged) -> Delivery {
        self.notify_pane(
            pane,
            protocol::notify::SELECTION_CHANGED,
            selection_params(&payload),
        )
    }

    /// Put a file reference into the prompt of the `claude` in `pane`.
    pub fn at_mentioned(&self, pane: &str, payload: AtMentioned) -> Delivery {
        self.notify_pane(
            pane,
            protocol::notify::AT_MENTIONED,
            mention_params(&payload),
        )
    }

    fn notify_pane(&self, pane: &str, method: &str, params: Value) -> Delivery {
        let Some(out) = self.inner.sender_for_pane(pane) else {
            let connections = self.inner.connection_count();
            tracing::debug!(
                pane,
                method,
                connections,
                "no connected claude in this pane; dropped"
            );
            return Delivery::NoConnection { connections };
        };
        send_json(&out, &Notification::new(method, params));
        Delivery::Sent
    }

    /// Stop accepting, resolve everything still waiting, and take the lockfile down.
    pub async fn shutdown(self) {
        self.accept.abort();

        // Before anything is torn down, so that a socket still inside its handshake cannot
        // register behind the sweep below and outlive the server.
        self.inner.conns.lock().closed = true;

        // Resolve first, close second. A `claude` blocked on `openDiff` has to receive its
        // rejection over a socket that is still open, or its turn ends with a transport
        // error where a plain "the user did not accept it" was available.
        let resolved = self.inner.broker.cancel_all(CancelReason::Shutdown);
        if resolved > 0 {
            tracing::info!(resolved, "rejected pending diffs on shutdown");
        }

        // Dropping the registry's senders is what closes the sockets — but only once the
        // handler tasks woken above have dropped their own clones, which they do after
        // writing their replies. The ordering falls out of the channel rather than a sleep.
        self.inner.conns.lock().open.clear();

        // And then wait for those writes to actually leave. The caller's next act is to
        // signal the `claude` children, so returning while a `DIFF_REJECTED` is still queued
        // hands the agent the transport error the ordering above exists to avoid.
        drop(self.inner.alive.lock().take());
        let drained = self.drained.lock().take();
        if let Some(mut rx) = drained
            && tokio::time::timeout(DRAIN_TIMEOUT, rx.recv())
                .await
                .is_err()
        {
            tracing::warn!(
                timeout = ?DRAIN_TIMEOUT,
                "an IDE connection did not finish writing; closing it anyway"
            );
        }

        // `self` drops here, and with it the lockfile: `~/.claude/ide/<port>.lock` goes away.
        // A lockfile outliving its server is worse than none, because the next CLI connects
        // to a dead port and reports a broken IDE rather than no IDE.
    }
}

impl Drop for IdeServer {
    fn drop(&mut self) {
        // A server dropped without `shutdown` would otherwise leave its accept loop running
        // for the life of the process, holding the port and answering for a project that is
        // gone.
        self.accept.abort();

        // The same wind-down `shutdown` performs, minus the part that needs to await. Drop is
        // reachable on any early return that built a server and then failed, and a `claude`
        // left blocked on `openDiff` by one of those has no other way to be told no: the
        // lockfile is about to vanish with `self`, so nothing will ever answer it.
        self.inner.conns.lock().closed = true;
        self.inner.broker.cancel_all(CancelReason::Shutdown);
        self.inner.conns.lock().open.clear();
        drop(self.inner.alive.lock().take());
    }
}

/// Re-shape a selection for the wire.
///
/// The CLI reads `params.selection.start.line` / `.end.line` and ignores any flat
/// `startLine`/`endLine`, so [`SelectionChanged`]'s fields are rebuilt here rather than
/// serialised straight through: the flat form parses cleanly at the other end and then does
/// nothing at all.
///
/// `end.character` is deliberately non-zero. The CLI counts `end.line - start.line + 1` and
/// subtracts one when `end.character` is 0, following the editor convention that a selection
/// stopping at column 0 does not include that line. Our `end_line` is inclusive, so a zero
/// here reports one line too few.
fn selection_params(payload: &SelectionChanged) -> Value {
    let end_character = payload
        .text
        .lines()
        .next_back()
        .map(|line| line.encode_utf16().count())
        .unwrap_or(0)
        .max(1);

    json!({
        "selection": {
            "start": { "line": payload.start_line, "character": 0 },
            "end": { "line": payload.end_line, "character": end_character },
        },
        "text": payload.text,
        "filePath": payload.file_path,
    })
}

/// Re-shape an at-mention for the wire.
///
/// An absent line number is **omitted** rather than sent as `null`. The CLI validates this
/// notification against a schema whose `lineStart`/`lineEnd` are optional but not nullable, and
/// a notification that fails validation is discarded with nothing on screen — so a whole-file
/// mention, which is [`AtMentioned`]'s documented `None` case, would do nothing at all.
/// Serialising [`AtMentioned`] directly writes `null` for those fields, which is why the object
/// is built by hand here.
fn mention_params(payload: &AtMentioned) -> Value {
    let mut params = json!({ "filePath": payload.file_path });
    if let Some(start) = payload.line_start {
        params["lineStart"] = json!(start);
    }
    if let Some(end) = payload.line_end {
        params["lineEnd"] = json!(end);
    }
    params
}

async fn accept_loop(listener: TcpListener, inner: Arc<Inner>) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                // Nagle costs a round-trip of latency on every small JSON-RPC frame, and
                // every frame here is small.
                if let Err(e) = stream.set_nodelay(true) {
                    tracing::debug!(error = %e, "could not disable Nagle on an IDE connection");
                }
                tokio::spawn(serve(stream, peer.to_string(), Arc::clone(&inner)));
            }
            Err(e) => {
                // Some accept errors are per-connection and some are permanent. Sleeping
                // briefly keeps a permanent one from becoming a busy loop that pins a core
                // for as long as the project stays open.
                tracing::warn!(error = %e, "accepting an IDE connection failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

async fn serve(stream: tokio::net::TcpStream, peer: String, inner: Arc<Inner>) {
    let token = inner.auth_token.clone();
    let handshake_peer = peer.clone();
    let upgrade = tokio_tungstenite::accept_hdr_async(
        stream,
        move |request: &HandshakeRequest, mut response: HandshakeResponse| {
            if let Some(denial) = authorize(request, &token, &handshake_peer) {
                return Err(denial);
            }

            // Only echo a subprotocol the client offered; answering with one it did not ask
            // for makes a conforming client abort the handshake.
            if offers_subprotocol(request) {
                response.headers_mut().insert(
                    "sec-websocket-protocol",
                    HeaderValue::from_static(protocol::SUBPROTOCOL),
                );
            }
            Ok(response)
        },
    )
    .await;

    let mut ws = match upgrade {
        Ok(ws) => ws,
        Err(e) => {
            tracing::debug!(peer = %peer, error = %e, "IDE handshake did not complete");
            return;
        }
    };

    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
    let Some(connection) = inner.register(out_tx) else {
        tracing::debug!(peer = %peer, "the IDE server shut down mid-handshake; refusing");
        let _ = ws.close(None).await;
        return;
    };
    // Held for as long as this task can still write, so that `shutdown` can wait for the
    // frames it produced rather than assume them.
    let alive = inner.alive.lock().clone();
    tracing::info!(connection, peer = %peer, "a claude connected to the IDE server");

    loop {
        tokio::select! {
            // Note the absence of an `out_tx` clone in this scope. The only long-lived
            // sender lives in the registry, so clearing the registry ends this loop — after
            // the in-flight handler tasks holding their own clones have written their
            // replies and dropped them.
            outgoing = out_rx.recv() => match outgoing {
                Some(message) => {
                    if ws.send(message).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
            incoming = ws.next() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    let Some(out) = inner.sender_for(connection) else {
                        break;
                    };
                    let inner = Arc::clone(&inner);
                    let text = text.to_string();
                    // Its own task: `openDiff` waits for a human, and this loop is the only
                    // thing that can deliver the `close_tab` that ends that wait.
                    tokio::spawn(async move { dispatch(&inner, connection, &text, &out).await });
                }
                // MCP is JSON text. A binary frame is not something to guess at.
                Some(Ok(Message::Binary(_))) => {
                    tracing::debug!(connection, "ignoring a binary frame");
                }
                Some(Ok(Message::Close(_))) | None => break,
                Some(Ok(_)) => {}
                Some(Err(e)) => {
                    tracing::debug!(connection, error = %e, "IDE connection failed");
                    break;
                }
            },
        }
    }

    let _ = ws.close(None).await;
    // Everything this connection had to say is now on the wire, which is all `shutdown`
    // waits for. The bookkeeping below is local and must not hold it up.
    drop(alive);

    inner.unregister(connection);
    // A `claude` that died mid-diff must not leave a request nothing will ever answer.
    let orphaned = inner
        .broker
        .cancel_connection(connection, CancelReason::Disconnected);
    tracing::info!(connection, orphaned, "a claude disconnected");
    inner.emit(ServerEvent::Disconnected { connection }).await;
}

/// Check the `X-Claude-Code-Ide-Authorization` header against the lockfile's token.
///
/// The comparison is constant-time. The token is the only thing standing between any local
/// process and an RPC channel that opens diffs over arbitrary paths, and a byte-by-byte
/// comparison against an attacker's guess leaks its prefix through how long the rejection
/// takes — recoverable one byte at a time by a process on the same machine, which is exactly
/// the attacker this file has.
///
/// Returns the 401 to send back, or `None` when the token matches.
fn authorize(request: &HandshakeRequest, token: &str, peer: &str) -> Option<ErrorResponse> {
    let presented = request
        .headers()
        .get(protocol::AUTH_HEADER)
        .map(|v| v.as_bytes())
        .unwrap_or(b"");

    if bool::from(presented.ct_eq(token.as_bytes())) {
        return None;
    }

    // Logged, because a rejection here means something on this machine is probing the port,
    // and an unexplained "the IDE never connected" is otherwise indistinguishable from it.
    tracing::warn!(
        peer,
        presented = presented.is_empty().then_some("nothing"),
        "rejected an IDE connection: bad authorization"
    );

    let mut denial = ErrorResponse::new(Some("unauthorized".to_string()));
    *denial.status_mut() = StatusCode::UNAUTHORIZED;
    Some(denial)
}

fn offers_subprotocol(request: &HandshakeRequest) -> bool {
    request
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|offered| {
            offered
                .split(',')
                .any(|p| p.trim().eq_ignore_ascii_case(protocol::SUBPROTOCOL))
        })
}

fn send_json<T: serde::Serialize>(out: &mpsc::UnboundedSender<Message>, message: &T) {
    match serde_json::to_string(message) {
        // A closed channel means the connection went away between deciding to answer and
        // answering, which is ordinary rather than exceptional.
        Ok(text) => {
            let _ = out.send(Message::text(text));
        }
        Err(e) => tracing::warn!(error = %e, "could not encode an outbound message"),
    }
}

async fn dispatch(
    inner: &Arc<Inner>,
    connection: u64,
    text: &str,
    out: &mpsc::UnboundedSender<Message>,
) {
    let incoming: Incoming = match serde_json::from_str(text) {
        Ok(m) => m,
        Err(e) => {
            // No id means no one to answer, so there is nothing to do but say so in the log.
            tracing::debug!(connection, error = %e, "ignoring an unparseable frame");
            return;
        }
    };

    let Some(id) = incoming.id.clone() else {
        notification(inner, connection, &incoming).await;
        return;
    };

    let response = request(inner, connection, &incoming, id).await;
    send_json(out, &response);
}

async fn notification(inner: &Arc<Inner>, connection: u64, incoming: &Incoming) {
    if incoming.method == protocol::notify::IDE_CONNECTED {
        match serde_json::from_value::<protocol::IdeConnected>(incoming.params.clone()) {
            Ok(hello) => {
                inner.set_pid(connection, hello.pid);
                let client_version = inner.client_version(connection);
                inner
                    .emit(ServerEvent::Connected {
                        connection,
                        pid: hello.pid,
                        client_version,
                    })
                    .await;
            }
            Err(e) => {
                // The connection still works; it just cannot be attributed to a pane, so
                // diffs will show without a source and selections will not reach it.
                tracing::warn!(connection, error = %e, "ide_connected carried no usable pid");
            }
        }
        return;
    }

    tracing::debug!(
        connection,
        method = %incoming.method,
        "ignoring a notification we do not act on"
    );
}

async fn request(inner: &Arc<Inner>, connection: u64, incoming: &Incoming, id: Value) -> Response {
    match incoming.method.as_str() {
        "initialize" => {
            // Read for the record before the reply is built. `initialize_result` deliberately
            // still echoes only `protocolVersion` — inventing one ends the handshake, see its
            // own comment — so this is a side effect on the connection rather than anything
            // the client is told about.
            inner.set_client_version(connection, client_version_of(&incoming.params));
            Response::ok(id, initialize_result(&incoming.params))
        }
        "tools/list" => Response::ok(id, json!({ "tools": tools::descriptors() })),
        "tools/call" => call_tool(inner, connection, &incoming.params, id).await,
        "ping" => Response::ok(id, json!({})),
        // Answered rather than fatal. The CLI's SDK probes for capabilities we do not have —
        // prompts, resources, completions — and closing the socket over one would take the
        // diff view down with it. The rule for this milestone is that a protocol surprise
        // degrades the diff view and never breaks the terminal.
        other => {
            tracing::debug!(connection, method = other, "unimplemented MCP method");
            Response::err(id, METHOD_NOT_FOUND, format!("no such method: {other}"))
        }
    }
}

/// `clientInfo.version` out of an `initialize`'s params.
///
/// A free function with its own test rather than a chain inline at the call site: the path is
/// two levels deep into untrusted-shaped JSON, and every way of getting it wrong — the wrong
/// key, a number instead of a string, a `clientInfo` that is not an object — produces `None`,
/// which is indistinguishable from a client that did not send one. That is the right failure
/// mode and precisely why it needs to be checked rather than eyeballed.
fn client_version_of(params: &Value) -> Option<&str> {
    params
        .get("clientInfo")?
        .get("version")
        .and_then(Value::as_str)
}

fn initialize_result(params: &Value) -> Value {
    let version = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or(DEFAULT_PROTOCOL_VERSION);

    json!({
        "protocolVersion": version,
        // The CLI logs the capabilities it saw and gates `tools/list` on this being present.
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": lockfile::IDE_NAME, "version": env!("CARGO_PKG_VERSION") },
    })
}

async fn call_tool(inner: &Arc<Inner>, connection: u64, params: &Value, id: Value) -> Response {
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Response::err(id, INVALID_PARAMS, "tools/call without a tool name");
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    let events = inner.events.lock().clone();
    let broker = &inner.broker;

    let result = match name {
        protocol::tool::OPEN_DIFF => {
            let pane = inner.pane_for(connection);
            tools::open_diff(&arguments, broker, connection, pane, &events).await
        }
        protocol::tool::CLOSE_TAB => tools::close_tab(&arguments, broker, &events).await,
        protocol::tool::CLOSE_ALL_DIFF_TABS => {
            tools::close_all_diff_tabs(broker, connection, &events).await
        }
        protocol::tool::GET_DIAGNOSTICS => {
            // Cloned out from under the lock, matching `Inner::emit`'s discipline: a
            // `parking_lot` guard held across the call would be held while the store's own lock
            // is taken, which is two locks in an order nothing else in this crate establishes.
            let source = inner.diagnostics.lock().clone();
            tools::get_diagnostics(&arguments, source.as_deref())
        }
        protocol::tool::OPEN_FILE => tools::open_file(&arguments, &events).await,
        // A JSON-RPC error rather than a result: the caller asked for something that is not
        // in `tools/list`, which is a mistake about the server rather than about the work.
        other => {
            return Response::err(id, METHOD_NOT_FOUND, format!("no such tool: {other}"));
        }
    };

    Response::ok(id, result.to_json())
}

#[cfg(test)]
mod live {
    //! One check against the *real* CLI rather than against our idea of it.
    //!
    //! Everything the rest of this file tests, it tests by talking to itself. That cannot
    //! catch the two things most likely to be wrong about an undocumented protocol: whether
    //! the CLI accepts our `initialize` reply, and whether it recognises the lockfile at all.
    //! Both were wrong at least once during this milestone, and both were found here.
    //!
    //! Ignored by default. It needs `claude` and `script(1)` on `PATH`, a working login, and
    //! it writes into the user's real `~/.claude/ide`. Run it with
    //! `cargo test -p cide-ide-mcp -- --ignored --nocapture`.
    //!
    //! `script(1)` rather than a bare spawn because the IDE client is a TUI feature: `-p`
    //! mode never connects, however the environment is set. The child needs a pty.

    use std::process::Stdio;
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    #[ignore = "spawns the real claude CLI"]
    async fn a_real_claude_finds_us_and_completes_the_handshake() {
        // The crate directory, not the test runner's: `claude` asks about trusting an unknown
        // folder before it does anything else, and a pty with nothing typed into it would sit
        // on that prompt until the timeout.
        let workspace = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let server = IdeServer::start(vec![workspace.clone()])
            .await
            .expect("the server starts and publishes a lockfile");
        let mut events = server.events();

        // `CLAUDE_CODE_SSE_PORT` both satisfies the CLI's lockfile validity check and forces
        // it to connect to this port rather than to whichever lockfile it would have picked.
        let mut child = tokio::process::Command::new("script")
            .args(["-qec", "claude", "/dev/null"])
            .current_dir(&workspace)
            .env("CLAUDE_CODE_SSE_PORT", server.port().to_string())
            .env("TERM", "xterm-256color")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("claude and script are on PATH");

        let connected = tokio::time::timeout(Duration::from_secs(45), events.recv()).await;

        // Reaped either way: a leaked `claude` outlives the test run holding a pty.
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;

        match connected {
            // Reaching this means the CLI got through the WebSocket handshake with our token
            // and subprotocol, accepted our `initialize` reply, read `tools/list`, and then
            // announced itself. Nothing short of the whole chain produces this event.
            Ok(Some(ServerEvent::Connected {
                pid,
                client_version,
                ..
            })) => {
                eprintln!(
                    "a real claude connected and named pid {pid}, version {}",
                    client_version.as_deref().unwrap_or("<none reported>")
                );
            }
            other => panic!(
                "no ide_connected from a real claude: {other:?}\n\
                 The usual cause is the lockfile's `transport`: the CLI decides WebSocket \
                 versus SSE on `transport == \"ws\"` and sends anything else to \
                 http://127.0.0.1:<port>/sse, which this crate does not serve."
            ),
        }

        server.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::net::TcpListener;
    use tokio_tungstenite::WebSocketStream;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    type Client = WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

    async fn server() -> IdeServer {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("an ephemeral loopback port");
        IdeServer::attach(listener, TOKEN.to_string()).expect("the server starts")
    }

    /// Connect the way the CLI does: subprotocol `mcp`, token in the header.
    async fn connect(
        port: u16,
        token: &str,
    ) -> std::result::Result<Client, tokio_tungstenite::tungstenite::Error> {
        let mut request = format!("ws://127.0.0.1:{port}/")
            .into_client_request()
            .expect("a well-formed ws url");
        request.headers_mut().insert(
            protocol::AUTH_HEADER,
            HeaderValue::from_str(token).expect("an ascii token"),
        );
        request.headers_mut().insert(
            "sec-websocket-protocol",
            HeaderValue::from_static(protocol::SUBPROTOCOL),
        );
        tokio_tungstenite::connect_async(request)
            .await
            .map(|(ws, _)| ws)
    }

    async fn send(ws: &mut Client, message: Value) {
        ws.send(Message::text(message.to_string()))
            .await
            .expect("the frame goes out");
    }

    /// Read frames until one carries a JSON-RPC id, so a notification in flight cannot be
    /// mistaken for the answer to a request.
    async fn reply(ws: &mut Client) -> Value {
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(5), ws.next())
                .await
                .expect("a reply within five seconds")
                .expect("the socket is open")
                .expect("a readable frame");
            if let Message::Text(text) = frame {
                let value: Value = serde_json::from_str(&text).expect("a JSON frame");
                if value.get("id").is_some() {
                    return value;
                }
            }
        }
    }

    async fn handshake(ws: &mut Client) {
        send(
            ws,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-06-18",
                "capabilities":{},
                "clientInfo":{"name":"claude-code","version":"2.1.226"}
            }}),
        )
        .await;
        let answer = reply(ws).await;
        assert_eq!(
            answer["result"]["protocolVersion"], "2025-06-18",
            "the server agrees with the client's version rather than proposing its own"
        );
        assert!(answer["result"]["capabilities"]["tools"].is_object());
    }

    /// The version the CLI names itself with survives from `initialize` to `Connected`.
    ///
    /// This is the wire fact behind the whole runtime half of the version record, and it is
    /// the join that nothing else covers: `initialize` is a *request* and `ide_connected` is a
    /// *notification* that arrives afterwards, so the version has to be parked on the
    /// connection in between. A build that read it into a local and dropped it would still
    /// pass every other test in this file.
    ///
    /// The `clientInfo` block is the one from the real 2.1.226 capture in
    /// `tests/real_cli.rs`, fields and all, rather than a minimal `{"version": "…"}`: what is
    /// being checked is that the right key is picked out of the shape the CLI actually sends.
    #[tokio::test]
    async fn the_version_the_cli_names_itself_with_reaches_the_connected_event() {
        let server = server().await;
        let mut events = server.events();

        let mut ws = connect(server.port(), TOKEN)
            .await
            .expect("the CLI connects");
        send(
            &mut ws,
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-11-25",
                "capabilities":{"roots":{"listChanged":true},"elicitation":{}},
                "clientInfo":{
                    "name":"claude-code",
                    "title":"Claude Code",
                    "version":"2.1.226",
                    "description":"Anthropic's agentic coding tool",
                    "websiteUrl":"https://claude.com/claude-code"
                }
            }}),
        )
        .await;
        let _ = reply(&mut ws).await;
        send(
            &mut ws,
            json!({"jsonrpc":"2.0","method":"ide_connected","params":{"pid":6001}}),
        )
        .await;

        match events.recv().await {
            Some(ServerEvent::Connected {
                pid,
                client_version,
                ..
            }) => {
                assert_eq!(pid, 6001);
                assert_eq!(
                    client_version.as_deref(),
                    Some("2.1.226"),
                    "the version was on the wire, was parsed, and did not survive the two                      frames between arriving and being announced"
                );
            }
            other => panic!("expected a Connected event, got {other:?}"),
        }

        server.shutdown().await;
    }

    /// A client that names no version is served, and says "unknown" rather than a version.
    ///
    /// `None` and "a version outside the range" are different answers with different
    /// remedies, and an empty string would be drawn on the Settings screen as though it were
    /// a version — which is why the setter filters blanks rather than storing what it got.
    #[tokio::test]
    async fn a_client_that_names_no_version_still_connects_and_reports_none() {
        let server = server().await;
        let mut events = server.events();

        let mut ws = connect(server.port(), TOKEN)
            .await
            .expect("the CLI connects");
        // Three shapes of nothing: no `clientInfo` at all, one without a `version`, and one
        // whose `version` is blank. All three have to answer `None`.
        for (id, params) in [
            (1, json!({"protocolVersion":"2025-06-18","capabilities":{}})),
            (
                2,
                json!({"protocolVersion":"2025-06-18","clientInfo":{"name":"someone"}}),
            ),
            (
                3,
                json!({"protocolVersion":"2025-06-18","clientInfo":{"name":"x","version":"  "}}),
            ),
        ] {
            send(
                &mut ws,
                json!({"jsonrpc":"2.0","id":id,"method":"initialize","params":params}),
            )
            .await;
            let _ = reply(&mut ws).await;
        }
        send(
            &mut ws,
            json!({"jsonrpc":"2.0","method":"ide_connected","params":{"pid":6002}}),
        )
        .await;

        assert!(
            matches!(
                events.recv().await,
                Some(ServerEvent::Connected {
                    pid: 6002,
                    client_version: None,
                    ..
                })
            ),
            "a client with no usable version must connect and report None, not an empty string"
        );

        server.shutdown().await;
    }

    #[test]
    fn the_version_is_read_out_of_the_shape_the_cli_actually_sends() {
        // Verbatim from the 2.1.226 capture.
        let real = json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {"roots": {"listChanged": true}, "elicitation": {}},
            "clientInfo": {
                "name": "claude-code",
                "title": "Claude Code",
                "version": "2.1.226",
                "description": "Anthropic's agentic coding tool",
                "websiteUrl": "https://claude.com/claude-code"
            }
        });
        assert_eq!(client_version_of(&real), Some("2.1.226"));

        // Every way of not being there answers the same `None`, which is why this needs a
        // test: the failures are all silent and all identical.
        for absent in [
            json!({}),
            json!({"clientInfo": {}}),
            json!({"clientInfo": null}),
            json!({"clientInfo": "claude-code"}),
            json!({"clientInfo": {"version": 2}}),
            // The neighbouring key, which is the plausible confusion.
            json!({"protocolVersion": "2025-11-25"}),
        ] {
            assert_eq!(client_version_of(&absent), None, "{absent}");
        }
    }

    #[tokio::test]
    async fn a_client_with_the_lockfile_token_is_let_in() {
        let server = server().await;
        let mut ws = connect(server.port(), TOKEN)
            .await
            .expect("the CLI connects");
        handshake(&mut ws).await;
        server.shutdown().await;
    }

    #[tokio::test]
    async fn a_wrong_token_of_the_same_length_is_refused() {
        let server = server().await;

        // Same length as the real token, differing in the last byte. This is the case the
        // constant-time comparison exists for: a byte-by-byte check answers a near-miss
        // measurably later than a first-byte miss, which is enough to recover the token one
        // byte at a time from another process on this machine.
        let near_miss = "0123456789abcdef0123456789abcdeg";
        assert_eq!(near_miss.len(), TOKEN.len());

        let refused = connect(server.port(), near_miss).await;
        assert!(refused.is_err(), "a wrong token must not reach the socket");

        // And the server is still serving, rather than having taken the rejection personally.
        let mut ws = connect(server.port(), TOKEN)
            .await
            .expect("the CLI connects");
        handshake(&mut ws).await;
        server.shutdown().await;
    }

    #[tokio::test]
    async fn a_missing_token_is_refused() {
        let server = server().await;

        let mut request = format!("ws://127.0.0.1:{}/", server.port())
            .into_client_request()
            .expect("a well-formed ws url");
        request.headers_mut().insert(
            "sec-websocket-protocol",
            HeaderValue::from_static(protocol::SUBPROTOCOL),
        );

        assert!(tokio_tungstenite::connect_async(request).await.is_err());
        server.shutdown().await;
    }

    #[tokio::test]
    async fn tools_list_offers_exactly_the_five_the_cli_calls() {
        let server = server().await;
        let mut ws = connect(server.port(), TOKEN)
            .await
            .expect("the CLI connects");
        handshake(&mut ws).await;

        send(
            &mut ws,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        )
        .await;
        let answer = reply(&mut ws).await;

        let listed = answer["result"]["tools"]
            .as_array()
            .expect("an array of tools");
        let names: Vec<&str> = listed.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(
            names,
            [
                "openDiff",
                "close_tab",
                "closeAllDiffTabs",
                "getDiagnostics",
                "openFile"
            ]
        );
        for tool in listed {
            assert_eq!(tool["inputSchema"]["type"], "object");
        }

        server.shutdown().await;
    }

    #[tokio::test]
    async fn an_unknown_method_is_answered_rather_than_fatal() {
        let server = server().await;
        let mut ws = connect(server.port(), TOKEN)
            .await
            .expect("the CLI connects");
        handshake(&mut ws).await;

        send(
            &mut ws,
            json!({"jsonrpc":"2.0","id":2,"method":"resources/list","params":{}}),
        )
        .await;
        let answer = reply(&mut ws).await;
        assert_eq!(answer["error"]["code"], METHOD_NOT_FOUND);

        // The socket survives it, which is the whole point: a capability probe must not take
        // the diff view down.
        send(&mut ws, json!({"jsonrpc":"2.0","id":3,"method":"ping"})).await;
        assert_eq!(reply(&mut ws).await["id"], 3);

        server.shutdown().await;
    }

    #[tokio::test]
    async fn an_unknown_tool_is_answered_rather_than_fatal() {
        let server = server().await;
        let mut ws = connect(server.port(), TOKEN)
            .await
            .expect("the CLI connects");
        handshake(&mut ws).await;

        send(
            &mut ws,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
                   "params":{"name":"getCurrentSelection","arguments":{}}}),
        )
        .await;
        assert_eq!(reply(&mut ws).await["error"]["code"], METHOD_NOT_FOUND);

        server.shutdown().await;
    }

    #[tokio::test]
    async fn get_diagnostics_answers_empty_over_the_wire_when_no_source_is_installed() {
        // The assertion is unchanged: this test server installs no `DiagnosticSource`, and a
        // server that was never given one still answers `[]`. Only the *reason* moved — see
        // `tools::get_diagnostics`.
        let server = server().await;
        let mut ws = connect(server.port(), TOKEN)
            .await
            .expect("the CLI connects");
        handshake(&mut ws).await;

        send(
            &mut ws,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
                   "params":{"name":"getDiagnostics","arguments":{"uri":"file:///f.rs"}}}),
        )
        .await;
        let answer = reply(&mut ws).await;

        assert_eq!(answer["result"]["isError"], false);
        // The CLI runs `JSON.parse` over `content[0].text` and iterates the result.
        assert_eq!(answer["result"]["content"][0]["text"], "[]");

        server.shutdown().await;
    }

    #[tokio::test]
    async fn get_diagnostics_carries_lsp_shaped_json_over_the_wire() {
        // The other half: with a source installed, the payload the CLI actually receives is
        // LSP-shaped text inside `content[0]`. Asserted over the socket rather than against the
        // tool alone, because the encoding into `content` is the part that has to survive.
        struct Fixed;
        impl tools::DiagnosticSource for Fixed {
            fn diagnostics(&self, _uri: Option<&str>) -> Vec<protocol::UriDiagnostics> {
                vec![protocol::UriDiagnostics {
                    uri: "file:///f.rs".into(),
                    diagnostics: vec![protocol::LspDiagnostic {
                        range: protocol::LspRange {
                            start: protocol::LspPosition {
                                line: 0,
                                character: 0,
                            },
                            end: protocol::LspPosition {
                                line: 0,
                                character: 3,
                            },
                        },
                        severity: 2,
                        code: None,
                        source: Some("gopls".into()),
                        message: "unused variable".into(),
                    }],
                }]
            }
        }

        let server = server().await;
        server.set_diagnostics(Arc::new(Fixed));
        let mut ws = connect(server.port(), TOKEN)
            .await
            .expect("the CLI connects");
        handshake(&mut ws).await;

        send(
            &mut ws,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
                   "params":{"name":"getDiagnostics","arguments":{}}}),
        )
        .await;
        let answer = reply(&mut ws).await;

        assert_eq!(answer["result"]["isError"], false);
        let text = answer["result"]["content"][0]["text"]
            .as_str()
            .expect("text");
        let parsed: Value = serde_json::from_str(text).expect("the CLI can JSON.parse this");
        assert_eq!(parsed[0]["uri"], "file:///f.rs");
        assert_eq!(parsed[0]["diagnostics"][0]["severity"], 2);
        assert_eq!(parsed[0]["diagnostics"][0]["message"], "unused variable");
        // Absent rather than null: `code` is `skip_serializing_if`, and a null would be a value
        // the CLI has to handle for no reason.
        assert!(parsed[0]["diagnostics"][0].get("code").is_none());

        server.shutdown().await;
    }

    #[tokio::test]
    async fn closing_an_unknown_tab_succeeds() {
        let server = server().await;
        let mut events = server.events();
        let mut ws = connect(server.port(), TOKEN)
            .await
            .expect("the CLI connects");
        handshake(&mut ws).await;

        send(
            &mut ws,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call",
                   "params":{"name":"close_tab","arguments":{"tab_name":"never-existed"}}}),
        )
        .await;
        let answer = reply(&mut ws).await;

        // The CLI sends this from `beforeExit` for tabs we may already have destroyed. An
        // error here puts a spurious failure in the transcript of a clean exit.
        assert_eq!(answer["result"]["isError"], false);
        assert!(answer["error"].is_null());

        // Drain so the events channel does not stall the connection task on shutdown.
        while events.try_recv().is_ok() {}
        server.shutdown().await;
    }

    #[tokio::test]
    async fn a_connection_names_its_pid_and_gets_its_own_pane() {
        let server = server().await;
        let mut events = server.events();

        server.bind_pane(4001, "pane-a".into());
        server.bind_pane(4002, "pane-b".into());

        let mut a = connect(server.port(), TOKEN)
            .await
            .expect("pane a connects");
        handshake(&mut a).await;
        send(
            &mut a,
            json!({"jsonrpc":"2.0","method":"ide_connected","params":{"pid":4001}}),
        )
        .await;
        assert!(matches!(
            events.recv().await,
            Some(ServerEvent::Connected { pid: 4001, .. })
        ));

        let mut b = connect(server.port(), TOKEN)
            .await
            .expect("pane b connects");
        handshake(&mut b).await;
        send(
            &mut b,
            json!({"jsonrpc":"2.0","method":"ide_connected","params":{"pid":4002}}),
        )
        .await;
        assert!(matches!(
            events.recv().await,
            Some(ServerEvent::Connected { pid: 4002, .. })
        ));

        server.selection_changed(
            "pane-b",
            SelectionChanged {
                file_path: "/src/main.rs".into(),
                text: "let x = 1;".into(),
                start_line: 3,
                end_line: 3,
            },
        );

        let frame = tokio::time::timeout(Duration::from_secs(5), b.next())
            .await
            .expect("pane b hears about it")
            .expect("the socket is open")
            .expect("a readable frame");
        let Message::Text(text) = frame else {
            panic!("expected a text frame");
        };
        let notification: Value = serde_json::from_str(&text).expect("a JSON frame");
        assert_eq!(notification["method"], "selection_changed");
        // The shape the CLI actually reads: a nested selection, not flat line numbers.
        assert_eq!(notification["params"]["selection"]["start"]["line"], 3);
        assert_eq!(notification["params"]["selection"]["end"]["line"], 3);
        assert_eq!(notification["params"]["filePath"], "/src/main.rs");

        // And pane a, which is a different agent in the same project, hears nothing. A
        // broadcast here would put one pane's selection into the other's prompt.
        assert!(
            tokio::time::timeout(Duration::from_millis(200), a.next())
                .await
                .is_err()
        );

        server.shutdown().await;
    }

    /// The routing question, asked of the same join that answers the delivery question.
    ///
    /// `addressable_panes` exists so `cmd::file` can rank its own panes and pick one that will
    /// actually receive — so the property that matters is not "it lists something", it is that
    /// it agrees with [`IdeServer::at_mentioned`] on every pane, in both directions. A
    /// predicate that over-reports routes a user's selection into a prompt that never gets it;
    /// one that under-reports refuses a send that would have worked and leaves the user with
    /// the error this whole change is removing.
    #[tokio::test]
    async fn a_pane_is_addressable_exactly_when_a_mention_to_it_would_land() {
        let server = server().await;
        let mut events = server.events();

        // Bound before anything connects, which is the ordinary order: the app learns the pid
        // when it spawns the child, and the child connects some milliseconds later.
        server.bind_pane(5001, "pane-a".into());
        server.bind_pane(5002, "pane-b".into());

        assert!(
            server.addressable_panes().is_empty(),
            "a binding is not a connection; a pane whose claude has not handshaken cannot \
             receive anything, and the resume-splash pane that started this bug report is \
             precisely this state"
        );

        let mut a = connect(server.port(), TOKEN)
            .await
            .expect("pane a connects");
        handshake(&mut a).await;
        send(
            &mut a,
            json!({"jsonrpc":"2.0","method":"ide_connected","params":{"pid":5001}}),
        )
        .await;
        assert!(matches!(
            events.recv().await,
            Some(ServerEvent::Connected { pid: 5001, .. })
        ));

        assert_eq!(
            server.addressable_panes(),
            vec!["pane-a".to_string()],
            "only the pane whose child is on the wire"
        );

        let mention = AtMentioned {
            file_path: "/src/main.rs".into(),
            line_start: Some(9),
            line_end: Some(19),
        };
        assert_eq!(
            server.at_mentioned("pane-a", mention.clone()),
            Delivery::Sent,
            "a pane this call reports must accept a mention, or the caller routes into a hole"
        );
        assert_eq!(
            server.at_mentioned("pane-b", mention.clone()),
            Delivery::NoConnection { connections: 1 },
            "and a pane it does not report must not — pane-b is bound, and bound is not enough"
        );

        // The unbind that had no caller. Its whole job is to stop a pid speaking for a pane
        // after the child is gone, so the observable effect is that the pane stops being
        // addressable while the connection is still open.
        server.unbind_pane(5001);
        assert!(
            server.addressable_panes().is_empty(),
            "unbinding takes the pane out of the routing table, which is the only way to \
             observe that `unbind_pane` does anything at all"
        );
        assert_eq!(
            server.at_mentioned("pane-a", mention),
            Delivery::NoConnection { connections: 1 },
            "and the two stay in agreement afterwards"
        );

        server.shutdown().await;
    }

    /// The wrapper case, from this crate's side of it.
    ///
    /// A `claude` that is not the process cide forked — launched through a shim, or typed into
    /// a shell pane — announces a pid nothing ever bound. Everything about that connection
    /// works except attribution: it counts towards `connection_count`, it receives broadcasts,
    /// and an addressed mention has nowhere to go. `unbound_pids` is how the application is
    /// told to go and find out whose child it is; the test is that the pid appears there
    /// exactly while it is unclaimed, and that binding it — which is all the resolution in
    /// `cide_app::ide` ultimately does — moves it into the routing table.
    #[tokio::test]
    async fn a_connection_nothing_bound_is_reported_until_something_binds_it() {
        let server = server().await;
        let mut events = server.events();

        // 7100 is the pty child cide forked (the wrapper). 7101 is the CLI it went on to
        // spawn, and 7101 is what reaches the socket.
        server.bind_pane(7100, "pane-a".into());

        let mut cli = connect(server.port(), TOKEN)
            .await
            .expect("the CLI connects");
        handshake(&mut cli).await;
        send(
            &mut cli,
            json!({"jsonrpc":"2.0","method":"ide_connected","params":{"pid":7101}}),
        )
        .await;
        assert!(matches!(
            events.recv().await,
            Some(ServerEvent::Connected { pid: 7101, .. })
        ));

        assert!(
            server.addressable_panes().is_empty(),
            "the pane is bound and a claude is connected, and still nothing can be addressed \
             — which is the whole of the reported bug"
        );
        assert_eq!(
            server.unbound_pids(),
            vec![7101],
            "and this is the question whose answer makes it fixable"
        );

        let mention = AtMentioned {
            file_path: "/src/main.rs".into(),
            line_start: Some(0),
            line_end: Some(0),
        };
        assert_eq!(
            server.at_mentioned("pane-a", mention.clone()),
            Delivery::NoConnection { connections: 1 },
            "one connection, none of it addressable: the count the error sentence prints"
        );

        // What `cide_app::ide` does with the answer, once the ancestry walk has said that
        // 7101 descends from 7100 and 7100 is pane-a's child.
        server.bind_pane(7101, "pane-a".into());

        assert!(
            server.unbound_pids().is_empty(),
            "nothing is unattributed any more"
        );
        assert_eq!(
            server.addressable_panes(),
            vec!["pane-a".to_string()],
            "and the pane became addressable without the CLI doing anything differently"
        );
        assert_eq!(
            server.at_mentioned("pane-a", mention),
            Delivery::Sent,
            "which is the send that used to fail"
        );

        server.shutdown().await;
    }

    #[tokio::test]
    async fn a_selection_for_a_pane_with_no_claude_is_dropped_quietly() {
        let server = server().await;
        server.selection_changed(
            "pane-with-nothing-in-it",
            SelectionChanged {
                file_path: "/f.rs".into(),
                text: String::new(),
                start_line: 0,
                end_line: 0,
            },
        );
        server.shutdown().await;
    }

    #[tokio::test]
    async fn a_dropped_connection_rejects_the_diff_it_was_waiting_on() {
        let server = server().await;
        let broker = server.broker();
        let mut events = server.events();
        let mut ws = connect(server.port(), TOKEN)
            .await
            .expect("the CLI connects");
        handshake(&mut ws).await;

        send(
            &mut ws,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "name":"openDiff",
                "arguments":{
                    "old_file_path":"/f.rs",
                    "new_file_path":"/f.rs",
                    "new_file_contents":"after",
                    "tab_name":"tab-1"
                }
            }}),
        )
        .await;

        let Some(ServerEvent::DiffRequested(_)) = events.recv().await else {
            panic!("the app is asked to open a tab");
        };
        assert_eq!(broker.len(), 1);

        // The child dies mid-diff. Nothing else will ever answer this request, so the socket
        // closing has to.
        drop(ws);
        let disconnected = tokio::time::timeout(Duration::from_secs(5), events.recv())
            .await
            .expect("a disconnect within five seconds");
        assert!(matches!(
            disconnected,
            Some(ServerEvent::Disconnected { .. })
        ));
        assert!(
            broker.is_empty(),
            "a dead claude must not leave a request nothing will answer"
        );

        server.shutdown().await;
    }

    #[tokio::test]
    async fn a_diff_is_answered_with_what_the_user_chose() {
        let server = server().await;
        let broker = server.broker();
        let mut events = server.events();
        let mut ws = connect(server.port(), TOKEN)
            .await
            .expect("the CLI connects");
        handshake(&mut ws).await;

        send(
            &mut ws,
            json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{
                "name":"openDiff",
                "arguments":{
                    "old_file_path":"/f.rs",
                    "new_file_path":"/f.rs",
                    "new_file_contents":"proposed",
                    "tab_name":"tab-1"
                }
            }}),
        )
        .await;

        let Some(ServerEvent::DiffRequested(request)) = events.recv().await else {
            panic!("the app is asked to open a tab");
        };

        // While that call is outstanding, the same connection stays responsive — the CLI
        // sends `close_tab` down this socket from its own abort handler.
        send(&mut ws, json!({"jsonrpc":"2.0","id":8,"method":"ping"})).await;
        assert_eq!(reply(&mut ws).await["id"], 8);

        broker.resolve(
            &request.id,
            crate::protocol::DiffOutcome::Saved {
                contents: "what the user left behind".into(),
            },
        );

        let answer = reply(&mut ws).await;
        assert_eq!(answer["id"], 7);
        assert_eq!(answer["result"]["content"][0]["text"], "FILE_SAVED");
        assert_eq!(
            answer["result"]["content"][1]["text"],
            "what the user left behind"
        );

        server.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_rejects_a_diff_rather_than_leaving_it_hanging() {
        let server = server().await;
        let broker = server.broker();
        let mut events = server.events();
        let mut ws = connect(server.port(), TOKEN)
            .await
            .expect("the CLI connects");
        handshake(&mut ws).await;

        send(
            &mut ws,
            json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{
                "name":"openDiff",
                "arguments":{
                    "old_file_path":"/f.rs",
                    "new_file_path":"/f.rs",
                    "new_file_contents":"proposed",
                    "tab_name":"tab-1"
                }
            }}),
        )
        .await;
        let Some(ServerEvent::DiffRequested(_)) = events.recv().await else {
            panic!("the app is asked to open a tab");
        };

        server.shutdown().await;

        // The rejection reaches the wire before the socket closes. Without that, the agent's
        // turn ends with a transport error instead of a decision it can report.
        let answer = reply(&mut ws).await;
        assert_eq!(answer["id"], 9);
        assert_eq!(answer["result"]["content"][0]["text"], "DIFF_REJECTED");
        assert!(broker.is_empty());
    }

    #[tokio::test]
    async fn an_inclusive_end_line_survives_the_cli_arithmetic() {
        // The CLI computes `end.line - start.line + 1` and subtracts one when
        // `end.character` is zero. A single-line selection must therefore not report zero.
        let params = selection_params(&SelectionChanged {
            file_path: "/f.rs".into(),
            text: "let x = 1;".into(),
            start_line: 7,
            end_line: 7,
        });

        let start = params["selection"]["start"]["line"]
            .as_i64()
            .expect("a start line");
        let end = params["selection"]["end"]["line"]
            .as_i64()
            .expect("an end line");
        let character = params["selection"]["end"]["character"]
            .as_i64()
            .expect("an end character");

        let counted = end - start + 1 - i64::from(character == 0);
        assert_eq!(counted, 1, "the CLI would report the wrong number of lines");
    }

    #[tokio::test]
    async fn an_at_mention_keeps_the_wire_spelling() {
        let server = server().await;
        server.bind_pane(5001, "pane-a".into());

        let mut ws = connect(server.port(), TOKEN)
            .await
            .expect("the CLI connects");
        handshake(&mut ws).await;
        send(
            &mut ws,
            json!({"jsonrpc":"2.0","method":"ide_connected","params":{"pid":5001}}),
        )
        .await;
        let mut events = server.events();
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("a connect event"),
            Some(ServerEvent::Connected { pid: 5001, .. })
        ));

        server.at_mentioned(
            "pane-a",
            AtMentioned {
                file_path: "/src/main.rs".into(),
                line_start: Some(9),
                line_end: Some(19),
            },
        );

        let frame = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("the mention arrives")
            .expect("the socket is open")
            .expect("a readable frame");
        let Message::Text(text) = frame else {
            panic!("expected a text frame");
        };
        let notification: Value = serde_json::from_str(&text).expect("a JSON frame");
        assert_eq!(notification["method"], "at_mentioned");
        // camelCase, and 0-based: the CLI adds one before it shows the user `@file#L10-20`.
        assert_eq!(notification["params"]["filePath"], "/src/main.rs");
        assert_eq!(notification["params"]["lineStart"], 9);
        assert_eq!(notification["params"]["lineEnd"], 19);

        server.shutdown().await;
    }

    /// A mention that reaches nobody has to *say* so.
    ///
    /// Both counts are asserted because they are two different sentences in the UI: zero
    /// connections is "no Claude in this project has connected to cide", a non-zero count is
    /// "the plumbing works and this pane is the odd one out". Answering `()` — which is what
    /// this did — leaves the frontend unable to tell either of them from success.
    #[tokio::test]
    async fn a_mention_with_no_claude_in_that_pane_reports_rather_than_vanishing() {
        let server = server().await;

        assert_eq!(
            server.at_mentioned(
                "pane-nobody-is-in",
                AtMentioned {
                    file_path: "/src/main.rs".into(),
                    line_start: Some(9),
                    line_end: Some(19),
                },
            ),
            Delivery::NoConnection { connections: 0 },
            "nothing is connected to this project at all"
        );

        let mut events = server.events();
        let mut ws = connect(server.port(), TOKEN)
            .await
            .expect("the CLI connects");
        handshake(&mut ws).await;
        send(
            &mut ws,
            json!({"jsonrpc":"2.0","method":"ide_connected","params":{"pid":8001}}),
        )
        .await;
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("a connect event"),
            Some(ServerEvent::Connected { pid: 8001, .. })
        ));

        assert_eq!(
            server.at_mentioned(
                "pane-nobody-is-in",
                AtMentioned {
                    file_path: "/src/main.rs".into(),
                    line_start: None,
                    line_end: None,
                },
            ),
            Delivery::NoConnection { connections: 1 },
            "one session is connected, just not that pane's"
        );

        server.bind_pane(8001, "pane-a".into());
        assert_eq!(
            server.at_mentioned(
                "pane-a",
                AtMentioned {
                    file_path: "/src/main.rs".into(),
                    line_start: None,
                    line_end: None,
                },
            ),
            Delivery::Sent,
        );

        server.shutdown().await;
    }

    #[test]
    fn a_whole_file_mention_omits_the_line_numbers_rather_than_nulling_them() {
        let params = mention_params(&AtMentioned {
            file_path: "/src/lib.rs".into(),
            line_start: None,
            line_end: None,
        });

        // Not `null`: the CLI's schema makes these optional but not nullable, and a
        // notification it refuses is dropped in silence. `null` here is an @-mention gesture
        // that does nothing and reports nothing.
        assert_eq!(params["filePath"], "/src/lib.rs");
        assert!(params.get("lineStart").is_none());
        assert!(params.get("lineEnd").is_none());
    }
}
