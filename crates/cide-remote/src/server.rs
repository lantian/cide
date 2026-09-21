//! Bind, accept, authenticate, answer. (M72)
//!
//! One task per connection, no shared mutable state between them but the host and the device
//! list, and every call into cide on a blocking thread. The accept loop never waits on a
//! connection and a connection never waits on another — `cide_ide_mcp::server`'s shape, and
//! `agent_rpc`'s rule that the accept loop is never blocked on a tool call.
//!
//! # The handshake is a frame, not a header
//!
//! `cide-ide-mcp` authenticates in the WebSocket upgrade, with a bearer token in a request
//! header, because the client it serves is `claude` and `claude` sets one. This server's client
//! is React Native, whose `WebSocket` cannot reliably set request headers — and a browser cannot
//! at all — so the credential is the first *frame* instead. Two consequences worth stating:
//!
//! * A socket exists before it is authorised, so this module has to say exactly what an
//!   unauthorised socket may do. It may send one frame. [`ClientBody::Pair`] or
//!   [`ClientBody::Hello`], once; anything else closes it.
//! * When the sealed transport arrives the credential disappears entirely — authentication
//!   becomes *can you seal a frame this cide can open* — and nothing above this layer changes,
//!   because the shape is already "the first frame proves who you are".

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use cide_ipc::remote::{
    ClientBody, ClientFrame, PROTOCOL_VERSION, ServerBody, ServerFrame, error_kind,
};
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::protocol::Message;

use crate::RemoteError;
use crate::devices::DeviceStore;
use crate::host::RemoteHost;
use crate::seal::{Handshake, Mode, Opener, Sealer, StaticKey};

/// How long a socket may stay unauthenticated.
///
/// An open socket that has said nothing costs a task and a file descriptor, and on a listener
/// reachable from a network there is no reason to hold one indefinitely. Generous enough that a
/// phone waking its radio makes it comfortably.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);

/// The same, for a socket that is **pairing**.
///
/// Longer, and bounded by the thing it is waiting for rather than by patience: a device pairing
/// by a typed address derives six digits and then says nothing at all while a person walks to
/// the machine, finds the pairing window and compares them. Twenty seconds is a reasonable wait
/// for a packet and an unreasonable one for that, and dropping the socket underneath somebody
/// mid-comparison reports itself as *the connection failed* — a network problem, which it is
/// not, so nobody would think to try again more quickly.
///
/// [`crate::devices::CODE_TTL`] is the right bound because it is already the answer to "how
/// long may this take": the code is dead afterwards, so a socket outliving it is waiting for
/// something that can no longer happen. A margin is added for the round trip that follows the
/// answer, so the code expires on cide's clock rather than on this one — one place decides when
/// a pairing window has closed, and it is the one that minted it.
const PAIRING_TIMEOUT: Duration = Duration::from_secs(crate::devices::CODE_TTL.as_secs() + 15);

/// How long [`RemoteServer::shutdown`] waits for connections to finish.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

const EVENT_CAPACITY: usize = 32;

/// How many frames may be queued towards one device before it is told to resynchronise.
///
/// Bounded, and the bound is the point — see [`cide_ipc::remote::ServerBody::Desync`]. Deep
/// enough that an ordinary burst (a snapshot is three frames, a busy turn is a state change every
/// few hundred milliseconds) never trips it, shallow enough that a wedged socket is noticed in
/// seconds rather than in megabytes.
const OUTBOUND_CAPACITY: usize = 64;

/// How long a workspace change is held before the projections are re-read and pushed.
///
/// `cide://workspace-changed` fires once per accepted mutation, and a pane drag is a run of them.
/// Re-deriving every device's project and session lists on each one would be the 2.25 MB task
/// board's mistake in a new place; coalescing makes a drag cost one push.
const WORKSPACE_COALESCE: Duration = Duration::from_millis(250);

/// How often a watched screen is read.
///
/// Not the coalescer's 8 ms. That cadence is tuned for a local IPC feeding xterm.js, where the
/// cost of a frame is a memcpy; here a frame is JSON over Wi-Fi to a battery. Ten reads a second
/// is past the point where a person can tell, and each read that finds nothing sends nothing.
const SCREEN_TICK: Duration = Duration::from_millis(100);

/// How long a screen must be still before it is read lazily.
const SCREEN_IDLE_AFTER: Duration = Duration::from_secs(2);

/// One read in this many ticks, once a screen has gone still.
///
/// A terminal nobody is typing into is the common case, and it is the case a companion app is
/// left sitting on. Half a second of latency on the first character after a pause is not
/// noticeable; ten grid walks a second for an hour is.
const IDLE_EVERY: u64 = 5;

/// Capability names announced in [`ServerBody::Welcome`].
///
/// A device asking *can you do X* gets a better answer than one asking *are you new enough*, and
/// this is the list it asks against. It grows a name per milestone phase.
pub const FEATURES: &[&str] = &[
    "projects",
    "sessions",
    "awaiting",
    "events",
    "screen",
    "input",
    "acknowledge",
    "runControl",
    "dispatch",
    "tasks",
    "prompt",
];

/// Something that happened in cide, on its way to whichever devices care.
///
/// Handed to [`RemoteServer::notify`] by the application's own event surface — `cide-app`'s
/// `emit.rs` tees each `cide://` event it sends to windows — so there is still exactly one file
/// listing everything the outside world can be told.
///
/// What is **not** here is as deliberate as what is. `session-tool` is the fast editor-reload
/// path and a phone has no editor; `fs-changed`, `git-status`, `diagnostics` and `docker-changed`
/// describe surfaces a device does not draw. Leaving them out is what keeps this quiet during an
/// active turn, which is the only time it matters.
#[derive(Debug, Clone)]
pub enum RemoteEvent {
    /// A session moved between idle, busy, awaiting permission or exited.
    SessionState {
        session: cide_ipc::SessionId,
        state: cide_ipc::SessionState,
    },
    /// cide's whole belief about which sessions are waiting on the user.
    Awaiting {
        entries: Vec<cide_ipc::remote::AwaitingEntry>,
    },
    /// A project's roles or runs moved: dispatched, paused, resumed, finished. (M75)
    ///
    /// The project and nothing else, for `WorkspaceRev`'s reason — what a device is owed is a
    /// fresh projection, and re-reading the roster on every state change of every run would
    /// read the disk once per turn of every agent.
    AgentsChanged { project: cide_ipc::ProjectId },
    /// A project's task board moved. (M75)
    TasksChanged { project: cide_ipc::ProjectId },
    /// The tree changed. **The revision and nothing else** — see `cide-ipc`'s `remote` header for
    /// why a `Workspace` may not cross this wire. Each device re-asks for its projections, on a
    /// timer, and gets a projection back.
    WorkspaceRev { rev: u64 },
}

/// Something a connection did, for the panel to draw.
#[derive(Debug, Clone)]
pub enum ServerEvent {
    Connected {
        device: String,
        addr: String,
    },
    Disconnected {
        device: String,
    },
    /// A new device redeemed a pairing code.
    Paired {
        device: String,
        name: String,
    },
    /// A connection was refused. Kept as an event rather than only a log line because the panel
    /// is where somebody is *looking* when a phone will not connect.
    Refused {
        addr: String,
        why: String,
    },
    /// A device reached the pairing step and is waiting to be compared against.
    ///
    /// Carries no digits. The panel reads them from [`RemoteServer::pairing_attempt`] instead,
    /// so the number it draws is the one a connection *currently* holds rather than one that
    /// arrived in a queue and may since have been replaced — a stale short authentication
    /// string is the single worst value this feature could put on a screen, because it is the
    /// one a person is about to accept.
    Pairing {
        addr: String,
    },
    /// The pairing connection went away, redeemed or not.
    PairingEnded,
}

/// One live connection, as the fan-out sees it.
///
/// The read loop and the fan-out are different tasks writing to the same socket, so neither owns
/// the sink: both hand frames to a bounded queue that one writer task drains. That is what makes
/// "tell every subscribed device" a `try_send` per connection rather than a lock held across a
/// network write, and it is why a device on a train cannot stall the thread that noticed the
/// event.
struct Conn {
    id: u64,
    out: mpsc::Sender<ServerFrame>,
    /// Set when a frame was dropped because the queue was full; consumed by the writer, which
    /// emits [`ServerBody::Desync`] before whatever it dequeues next.
    ///
    /// A flag rather than a queued frame, because the queue being full is precisely the state in
    /// which another frame cannot be queued.
    desynced: Arc<AtomicBool>,
    /// A connection is in the table from the moment it exists so that closing it is one code
    /// path, and is only *served* once it has proved who it is.
    authenticated: Arc<AtomicBool>,
    subscribed: Arc<Mutex<Vec<cide_ipc::ProjectId>>>,
}

/// The screens one connection is watching. Empty for a device looking at a list, which is the
/// state that costs cide nothing at all.
type Watches = Arc<Mutex<std::collections::HashMap<cide_ipc::SessionId, crate::screen::Watch>>>;

/// One connection, detached from the table so a caller may await between frames.
struct Recipient {
    out: mpsc::Sender<ServerFrame>,
    desynced: Arc<AtomicBool>,
    subscribed: Vec<cide_ipc::ProjectId>,
}

struct Inner {
    host: Arc<dyn RemoteHost>,
    devices: Arc<DeviceStore>,
    /// This instance's long-lived key. Its public half is in every pairing payload; a device
    /// that cannot complete an exchange against it is not talking to this cide.
    statik: Arc<StaticKey>,
    events: Mutex<mpsc::Sender<ServerEvent>>,
    /// Dropped by each connection task; the receiver yields `None` once every clone is gone.
    alive: Mutex<Option<mpsc::UnboundedSender<()>>>,
    conns: Mutex<Vec<Conn>>,
    next_conn: AtomicU64,
    /// A workspace mutation arrived and the projections have not been re-read yet.
    workspace_dirty: Arc<AtomicBool>,
    /// Projects whose runs, roster or board moved and have not been re-read yet. (M75)
    ///
    /// A set rather than a flag because these reads are *per project* and re-reading every open
    /// project because one agent finished a turn would put a disk walk on the tracker of each.
    projects_dirty: Mutex<std::collections::HashSet<cide_ipc::ProjectId>>,
    /// The connection currently at the pairing step, and the digits it derived.
    ///
    /// One slot and not a list, because one code is outstanding at a time: a second device
    /// arriving while the first is deciding replaces what the panel shows, which is correct —
    /// the number on screen must always be the one belonging to the connection that would
    /// redeem the code, or it is worse than showing nothing.
    pairing: Mutex<Option<(u64, cide_ipc::remote::PairingAttempt)>>,
}

impl Inner {
    /// Offer a frame to every authenticated connection, letting each decide by what it asked for.
    ///
    /// `try_send` and never `send`. The callers reach here from [`RemoteServer::notify`], which is
    /// whichever of the application's own threads just emitted a `cide://` event — the hook
    /// applier and the GTK main loop among them. A `send().await` there would make a phone's
    /// socket a back-pressure path into the event surface of the whole IDE; dropping the frame
    /// and raising `desynced` is the trade, and the device is told.
    ///
    /// The connection table is locked across the loop, which is safe because nothing inside it
    /// waits: `try_send` returns immediately whether there is room or not.
    fn fan_out(&self, mut body_for: impl FnMut(&[cide_ipc::ProjectId]) -> Option<ServerBody>) {
        for conn in self.conns.lock().iter() {
            if !conn.authenticated.load(Ordering::Relaxed) {
                continue;
            }
            let subscribed = conn.subscribed.lock().clone();
            let Some(body) = body_for(&subscribed) else {
                continue;
            };
            if conn.out.try_send(ServerFrame { id: None, body }).is_err() {
                conn.desynced.store(true, Ordering::Relaxed);
            }
        }
    }

    /// Snapshot the connections a caller may then `await` between, without holding the table.
    ///
    /// [`Self::fan_out`] is the synchronous path and keeps the lock; this is for the two callers
    /// that have to suspend in the middle — the coalescer, which asks the host between frames,
    /// and shutdown. Holding a `parking_lot::Mutex` across an `.await` is the deadlock this
    /// avoids being able to write.
    fn recipients(&self) -> Vec<Recipient> {
        self.conns
            .lock()
            .iter()
            .filter(|c| c.authenticated.load(Ordering::Relaxed))
            .map(|c| Recipient {
                out: c.out.clone(),
                desynced: Arc::clone(&c.desynced),
                subscribed: c.subscribed.lock().clone(),
            })
            .collect()
    }

    fn forget_conn(&self, id: u64) {
        self.conns.lock().retain(|c| c.id != id);
    }
}

/// A bound, accepting listener.
pub struct RemoteServer {
    addr: SocketAddr,
    inner: Arc<Inner>,
    /// The accept loop and the workspace coalescer, in `Mutex`es so [`RemoteServer::shutdown`]
    /// can take and **await** them through a shared reference.
    ///
    /// Both halves of that are load-bearing, and the shape is what it is because the obvious
    /// one was wrong. `shutdown(self)` meant the caller had to own the server, which meant
    /// `Arc::try_unwrap`, which **fails whenever anything else holds a clone** — and the event
    /// pump holds one for as long as its channel is open. The losing arm dropped a refcount and
    /// called it a stop; the accept loop went on owning the `TcpListener` for the life of the
    /// process, so every later start answered *port already in use* and no amount of turning
    /// the feature off and on again could free it. A stop that can be skipped is not a stop.
    ///
    /// Awaiting rather than only aborting is the second half: `abort` requests cancellation and
    /// the listener is released when the runtime next drops the task, which is *after* a
    /// `reconcile` has already tried to re-bind. Joining makes "shutdown returned" mean "the
    /// port is free", which is exactly what the next line of `reconcile` assumes.
    accept: Mutex<Option<JoinHandle<()>>>,
    coalescer: Mutex<Option<JoinHandle<()>>>,
    events_rx: Mutex<Option<mpsc::Receiver<ServerEvent>>>,
    drained: Mutex<Option<mpsc::UnboundedReceiver<()>>>,
}

impl RemoteServer {
    /// Bind `addr` and start accepting.
    pub async fn bind(
        addr: SocketAddr,
        host: Arc<dyn RemoteHost>,
        devices: Arc<DeviceStore>,
        statik: Arc<StaticKey>,
    ) -> Result<Self, RemoteError> {
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| RemoteError::Bind(format!("{addr}: {e}")))?;
        Self::attach(listener, host, devices, statik)
    }

    /// Start on an already-bound listener. What the tests use, and what lets a caller bind port
    /// zero and ask afterwards which port it got.
    pub fn attach(
        listener: TcpListener,
        host: Arc<dyn RemoteHost>,
        devices: Arc<DeviceStore>,
        statik: Arc<StaticKey>,
    ) -> Result<Self, RemoteError> {
        let addr = listener
            .local_addr()
            .map_err(|e| RemoteError::Bind(e.to_string()))?;
        let (events_tx, events_rx) = mpsc::channel(EVENT_CAPACITY);
        let (alive_tx, drained_rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            host,
            devices,
            statik,
            events: Mutex::new(events_tx),
            alive: Mutex::new(Some(alive_tx)),
            conns: Mutex::new(Vec::new()),
            next_conn: AtomicU64::new(1),
            workspace_dirty: Arc::new(AtomicBool::new(false)),
            pairing: Mutex::new(None),
            projects_dirty: Mutex::new(std::collections::HashSet::new()),
        });
        let accept = tokio::spawn(accept_loop(listener, Arc::clone(&inner)));
        let coalescer = tokio::spawn(coalesce_workspace(Arc::clone(&inner)));
        Ok(Self {
            addr,
            inner,
            accept: Mutex::new(Some(accept)),
            coalescer: Mutex::new(Some(coalescer)),
            events_rx: Mutex::new(Some(events_rx)),
            drained: Mutex::new(Some(drained_rx)),
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// How many devices are connected right now. For the panel, and for tests.
    pub fn connected(&self) -> usize {
        self.inner
            .conns
            .lock()
            .iter()
            .filter(|c| c.authenticated.load(Ordering::Relaxed))
            .count()
    }

    /// Take the event stream. A stream has one consumer; see `cide_ide_mcp::server::events`.
    /// The connection currently waiting to be compared against, if there is one.
    ///
    /// Read rather than pushed, because a short authentication string is only ever correct in
    /// the present tense: a queued one that arrived behind a replacement is the number a person
    /// would accept for a connection that is not the one redeeming the code.
    #[must_use]
    pub fn pairing_attempt(&self) -> Option<cide_ipc::remote::PairingAttempt> {
        self.inner.pairing.lock().as_ref().map(|(_, a)| a.clone())
    }

    pub fn events(&self) -> mpsc::Receiver<ServerEvent> {
        if let Some(rx) = self.events_rx.lock().take() {
            return rx;
        }
        let (tx, rx) = mpsc::channel(EVENT_CAPACITY);
        *self.inner.events.lock() = tx;
        rx
    }

    /// Tell whichever devices care that something happened in cide.
    ///
    /// **Addressed, never broadcast.** A device hears about a session in a project it subscribed
    /// to, and about nothing else — `cide_ide_mcp::server`'s rule about `selection_changed`, for
    /// the same reason: a message aimed at everything is a message that reaches the wrong place.
    ///
    /// Synchronous and non-blocking, because the caller is whichever thread just emitted a
    /// `cide://` event and it must not be made to wait on a network.
    pub fn notify(&self, event: RemoteEvent) {
        match event {
            RemoteEvent::WorkspaceRev { .. } => {
                // Not sent on. The tree cannot cross this wire, so what a device is owed is a
                // fresh *projection*, and re-deriving one per mutation would make a pane drag
                // expensive. The coalescer picks this up.
                self.inner.workspace_dirty.store(true, Ordering::Relaxed);
            }
            RemoteEvent::SessionState { session, state } => {
                // Not narrowed by subscription: a device's notification rule and its "what is in
                // progress" screen both read across every session it can see.
                self.inner
                    .fan_out(|_| Some(ServerBody::SessionState { session, state }));
            }
            RemoteEvent::AgentsChanged { project } | RemoteEvent::TasksChanged { project } => {
                // Marked, not sent: these are three host reads and one of them walks a tracker,
                // and an agent mid-turn emits this many times a second. The coalescer decides.
                self.inner.projects_dirty.lock().insert(project);
            }
            RemoteEvent::Awaiting { entries } => {
                self.inner.fan_out(|_| {
                    Some(ServerBody::Awaiting {
                        entries: entries.clone(),
                    })
                });
            }
        }
    }

    /// Stop accepting, then wait briefly for live connections to end.
    ///
    /// The wait is what makes a quit say *going away* rather than dropping every socket: a device
    /// that is told will say "the machine went away", and one that is not says "something went
    /// wrong", and those are different sentences to read on a phone at the end of the day.
    /// Takes `&self` on purpose — see the field's own note. Calling it twice is harmless: the
    /// handles are gone after the first, and every other step is idempotent.
    pub async fn shutdown(&self) {
        let accept = self.accept.lock().take();
        let coalescer = self.coalescer.lock().take();
        for handle in [&accept, &coalescer].into_iter().flatten() {
            handle.abort();
        }
        for conn in self.inner.recipients() {
            let _ = conn.out.try_send(ServerFrame {
                id: None,
                body: ServerBody::GoingAway {
                    why: "cide is shutting down".to_owned(),
                },
            });
        }
        // Dropping the last sender is what makes the receiver finish. It is held in a `Mutex`
        // rather than owned so `shutdown` can take it without `self` being `mut`.
        self.inner.alive.lock().take();
        let drained = self.drained.lock().take();
        if let Some(mut drained) = drained {
            let _ = tokio::time::timeout(DRAIN_TIMEOUT, drained.recv()).await;
        }

        // Joined last, and the reason is the whole point of this function: an aborted task has
        // only been *asked* to stop, and the `TcpListener` it owns is released when it is
        // dropped. Waiting here is what lets the caller re-bind the same port on the next line.
        // A cancelled join is an `Err` and is the expected outcome, not a failure.
        for handle in [accept, coalescer].into_iter().flatten() {
            let _ = handle.await;
        }
    }
}

/// Re-read the projections and push them, at most once per [`WORKSPACE_COALESCE`].
///
/// One task for the whole server rather than one per connection: the read is the same read for
/// everybody, and doing it per device would multiply a workspace lock by the number of phones in
/// the house.
async fn coalesce_workspace(inner: Arc<Inner>) {
    let mut tick = tokio::time::interval(WORKSPACE_COALESCE);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tick.tick().await;
        let moved: Vec<cide_ipc::ProjectId> = {
            let mut dirty = inner.projects_dirty.lock();
            dirty.drain().collect()
        };
        let tree_moved = inner.workspace_dirty.swap(false, Ordering::Relaxed);
        if !tree_moved && moved.is_empty() {
            continue;
        }
        let conns = inner.recipients();
        if conns.is_empty() {
            continue;
        }

        // The per-project half first, and only to the devices actually subscribed to a project
        // that moved. A device watching one project pays nothing for an agent running in
        // another, which is `notify`'s addressed-never-broadcast rule applied to a read.
        for project in moved {
            let watching: Vec<_> = conns
                .iter()
                .filter(|conn| {
                    // Empty is *everything*, `sessions(None)`'s rule — a device watching the
                    // whole machine is watching this project too.
                    conn.subscribed.is_empty() || conn.subscribed.contains(&project)
                })
                .collect();
            if watching.is_empty() {
                continue;
            }
            let runs = ask(&inner, move |host| host.runs(project)).await;
            let roster = ask(&inner, move |host| host.roster(project)).await;
            let tasks = ask(&inner, move |host| host.board(project)).await;
            for conn in watching {
                for body in [
                    ServerBody::Runs {
                        project,
                        runs: runs.clone(),
                    },
                    ServerBody::Roster {
                        project,
                        agents: roster.agents.clone(),
                        dispatching: roster.dispatching,
                    },
                    ServerBody::Board {
                        project,
                        tasks: tasks.clone(),
                    },
                ] {
                    if conn.out.try_send(ServerFrame { id: None, body }).is_err() {
                        conn.desynced.store(true, Ordering::Relaxed);
                    }
                }
            }
        }

        if !tree_moved {
            continue;
        }
        let (rev, projects) = ask(&inner, |host| host.projects()).await;
        for conn in conns {
            let only = lone(&conn.subscribed);
            let sessions = ask(&inner, move |host| host.sessions(only)).await;
            for body in [
                ServerBody::Projects {
                    rev,
                    projects: projects.clone(),
                },
                ServerBody::Sessions { sessions },
            ] {
                if conn.out.try_send(ServerFrame { id: None, body }).is_err() {
                    conn.desynced.store(true, Ordering::Relaxed);
                }
            }
        }
    }
}

/// The one project a connection narrowed itself to, if it narrowed itself to exactly one.
///
/// Subscribing to several projects asks for everything, which is the honest reading: the host's
/// narrowing takes one project or none, and answering a two-project subscription with one
/// project's sessions would be silently wrong in the direction of *missing rows*.
fn lone(subscribed: &[cide_ipc::ProjectId]) -> Option<cide_ipc::ProjectId> {
    (subscribed.len() == 1).then(|| subscribed[0])
}

async fn accept_loop(listener: TcpListener, inner: Arc<Inner>) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                // Every frame here is small, so Nagle costs a round trip and buys nothing.
                if let Err(error) = stream.set_nodelay(true) {
                    tracing::debug!(%error, "remote: could not disable Nagle");
                }
                let alive = inner.alive.lock().clone();
                let inner = Arc::clone(&inner);
                tokio::spawn(async move {
                    let _alive = alive;
                    serve(stream, peer, inner).await;
                });
            }
            Err(error) => {
                // Some accept errors are per-connection and some are permanent; sleeping keeps a
                // permanent one from becoming a busy loop that pins a core for the life of the
                // application.
                tracing::warn!(%error, "remote: accepting a connection failed");
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    }
}

/// What a completed handshake established.
struct Agreed {
    mode: Mode,
    /// Empty in [`Mode::Pair`] — there is no device yet.
    device: String,
    channel: (Arc<Mutex<Opener>>, Sealer),
    /// The six digits this connection derived, for a person to compare against the phone's.
    ///
    /// Carried for every connection and reported for `Mode::Pair` only: a resumed connection
    /// has a pinned key and a token, so there is nothing for a person to adjudicate and a
    /// number on screen would only teach them to wave one through.
    sas: crate::seal::Sas,
}

/// Read the handshake and derive the channel, or close.
///
/// The one thing said in the clear here is a refusal, and only for a device this instance does
/// not know. That is deliberate: a revoked phone would otherwise get a socket that opens and then
/// goes silent, which is indistinguishable from a bad network — and "you were removed" is a
/// sentence somebody can act on. It reveals nothing, because the device id is already in the
/// handshake the client sent.
///
/// Everything else that can go wrong simply produces a channel the other end cannot use, which
/// is [`crate::seal`]'s whole design: a wrong key is not *detected*, it is not understood.
async fn shake_hands(
    sink: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        Message,
    >,
    stream: &mut futures_util::stream::SplitStream<
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    >,
    inner: &Arc<Inner>,
    addr: &str,
) -> Option<Agreed> {
    // The greeting goes out **before** the handshake is read, and nothing waits on it. The two
    // messages cross in flight: a handshake does not depend on `S` and a greeting does not
    // depend on `e`, so a device that needs the key to derive against has it by the time it has
    // anything to seal, and one that already had it pays nothing.
    //
    // It is sent to every connection rather than only to a pairing one, because refusing to
    // greet a resuming device would make the key's *absence* the signal for which mode this is
    // — readable off the wire by anyone watching, and a difference two implementations would
    // eventually disagree about.
    let greeting = crate::seal::Greeting {
        seal_version: crate::seal::SEAL_VERSION,
        server_public: inner.statik.public_bytes(),
    };
    if sink.send(Message::binary(greeting.encode())).await.is_err() {
        return None;
    }

    let first = match tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.next()).await {
        Ok(Some(Ok(message))) => message,
        Ok(_) => return None,
        Err(_) => {
            tracing::debug!(%addr, "remote: a connection said nothing and was dropped");
            return None;
        }
    };
    let Message::Binary(bytes) = first else {
        tracing::debug!(%addr, "remote: a connection opened with something that was not a handshake");
        return None;
    };
    let Some(handshake) = Handshake::decode(&bytes) else {
        tracing::debug!(%addr, "remote: an unreadable handshake");
        return None;
    };

    let psk: Vec<u8> = match handshake.mode {
        // No key yet, and none needed: this exchange authenticates the *server*, which is what
        // keeps the key it is about to hand over off the wire in the clear. The device is
        // authenticated by the code, inside.
        Mode::Pair => Vec::new(),
        Mode::Resume => match inner.devices.seal_key(&handshake.device) {
            Some(key) => key.to_vec(),
            None => {
                tracing::warn!(%addr, device = %handshake.device, "remote: an unknown device");
                note(
                    inner,
                    ServerEvent::Refused {
                        addr: addr.to_owned(),
                        why: "unknown device".to_owned(),
                    },
                );
                let refusal = ServerFrame {
                    id: None,
                    body: ServerBody::Error {
                        kind: error_kind::UNAUTHORIZED.to_owned(),
                        detail: "this device is not paired with this cide".to_owned(),
                    },
                };
                if let Ok(json) = serde_json::to_vec(&refusal) {
                    let _ = sink
                        .send(Message::text(String::from_utf8_lossy(&json).into_owned()))
                        .await;
                }
                return None;
            }
        },
    };

    let derived = crate::seal::derive(&inner.statik, &handshake, &psk, true);
    Some(Agreed {
        mode: handshake.mode,
        device: handshake.device,
        channel: (Arc::new(Mutex::new(derived.opener)), derived.sealer),
        sas: derived.sas,
    })
}

async fn serve(stream: tokio::net::TcpStream, peer: SocketAddr, inner: Arc<Inner>) {
    let addr = peer.to_string();
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(error) => {
            tracing::debug!(%error, %addr, "remote: a connection never became a websocket");
            return;
        }
    };

    let (mut sink, mut stream) = ws.split();

    // The handshake, in the clear, and the only thing this socket may say before it is sealed.
    // Everything in it is public — an ephemeral public key and a device id — and what it
    // establishes is not.
    let Some(agreed) = shake_hands(&mut sink, &mut stream, &inner, &addr).await else {
        return;
    };
    let (opener, sealer) = agreed.channel;

    let (out_tx, out_rx) = mpsc::channel(OUTBOUND_CAPACITY);
    let id = inner.next_conn.fetch_add(1, Ordering::Relaxed);

    // A pairing connection is the one a person is asked to adjudicate, so it is published the
    // moment it exists rather than when it speaks: the digits belong to the *exchange*, and a
    // device that derived them and then said nothing is exactly the case somebody at the panel
    // needs to be able to see and refuse.
    if agreed.mode == Mode::Pair {
        *inner.pairing.lock() = Some((
            id,
            cide_ipc::remote::PairingAttempt {
                addr: addr.clone(),
                sas: agreed.sas.grouped(),
            },
        ));
        note(&inner, ServerEvent::Pairing { addr: addr.clone() });
    }
    let desynced = Arc::new(AtomicBool::new(false));
    let authenticated = Arc::new(AtomicBool::new(false));
    let subscribed = Arc::new(Mutex::new(Vec::new()));
    let watches: Watches = Arc::new(Mutex::new(std::collections::HashMap::new()));
    inner.conns.lock().push(Conn {
        id,
        out: out_tx.clone(),
        desynced: Arc::clone(&desynced),
        authenticated: Arc::clone(&authenticated),
        subscribed: Arc::clone(&subscribed),
    });
    let writer = tokio::spawn(write_loop(sink, out_rx, Arc::clone(&desynced), sealer));
    let watcher = tokio::spawn(watch_loop(
        Arc::clone(&inner),
        out_tx.clone(),
        Arc::clone(&watches),
    ));

    let mut identified: Option<String> = None;
    // Namespaced so a device and a window can never collide in the write watermark's key, which
    // is `(session, writer)` and is a plain string on both sides.
    let mut writer_tag = format!("remote:{id}");

    loop {
        let next = if authenticated.load(Ordering::Relaxed) {
            stream.next().await
        } else {
            // A pairing socket is allowed to be quiet for as long as its code could still be
            // redeemed, because the silence is a person reading a number off another screen.
            let patience = if agreed.mode == Mode::Pair {
                PAIRING_TIMEOUT
            } else {
                HANDSHAKE_TIMEOUT
            };
            match tokio::time::timeout(patience, stream.next()).await {
                Ok(next) => next,
                Err(_) => {
                    tracing::debug!(%addr, "remote: a connection said nothing and was dropped");
                    break;
                }
            }
        };

        let Some(message) = next else { break };
        let message = match message {
            Ok(message) => message,
            Err(error) => {
                tracing::debug!(%error, %addr, "remote: a connection failed");
                break;
            }
        };

        let boxed = match message {
            Message::Binary(bytes) => bytes,
            Message::Close(_) => break,
            // Ping and Pong are the protocol's own and tungstenite answers them. A **text** frame
            // is a client that has not sealed anything, which after the handshake is either a bug
            // or somebody trying it on; either way there is nothing to say to it that would not
            // be said in the clear.
            _ => continue,
        };

        let plaintext = match opener.lock().open(&boxed) {
            Ok(plaintext) => plaintext,
            Err(error) => {
                tracing::warn!(%addr, %error, "remote: a frame did not open");
                break;
            }
        };

        let frame: ClientFrame = match serde_json::from_slice(&plaintext) {
            Ok(frame) => frame,
            Err(error) => {
                // Deliberately **not** fatal. A device built against a newer cide must degrade
                // rather than disconnect — `cide-ide-mcp`'s "never break the terminal" rule, one
                // surface over — so an unparseable frame is answered and the socket stays.
                tracing::debug!(%error, %addr, "remote: an unreadable frame");
                if say(
                    &out_tx,
                    envelope_id(&plaintext),
                    ServerBody::Error {
                        kind: error_kind::UNKNOWN_FRAME.to_owned(),
                        detail: "this cide does not understand that frame".to_owned(),
                    },
                )
                .await
                .is_err()
                {
                    break;
                }
                continue;
            }
        };

        let id_of = frame.id;
        let authorised = authenticated.load(Ordering::Relaxed);
        match (authorised, frame.body) {
            // --- the one frame an unauthenticated socket may send -----------------------
            (false, ClientBody::Pair { code, client }) => {
                if agreed.mode != Mode::Pair {
                    let _ = say(
                        &out_tx,
                        id_of,
                        ServerBody::Error {
                            kind: error_kind::UNEXPECTED.to_owned(),
                            detail: "this connection was opened to resume, not to pair".to_owned(),
                        },
                    )
                    .await;
                    break;
                }
                match inner.devices.redeem(&code, &client.name, &client.platform) {
                    Ok((device, key)) => {
                        note(
                            &inner,
                            ServerEvent::Paired {
                                device: device.clone(),
                                name: client.name.clone(),
                            },
                        );
                        // Who this is, said here because a device that pairs by a typed
                        // address has never been told and has nothing to invent it from.
                        let instance = ask(&inner, |host| host.instance()).await;
                        let _ = say(
                            &out_tx,
                            id_of,
                            ServerBody::Paired {
                                device,
                                key,
                                instance: instance.id,
                                label: instance.name,
                            },
                        )
                        .await;
                        // Paired, and now it must say hello like anybody else. Treating the pair
                        // as a login would mean two roads into the authenticated state, and the
                        // second one would be the one nobody re-reads.
                    }
                    Err(error) => {
                        let why = error.to_string();
                        note(
                            &inner,
                            ServerEvent::Refused {
                                addr: addr.clone(),
                                why: why.clone(),
                            },
                        );
                        tracing::warn!(%addr, %why, "remote: a pairing attempt was refused");
                        let _ = say(
                            &out_tx,
                            id_of,
                            ServerBody::Error {
                                kind: error_kind::PAIRING.to_owned(),
                                detail: why,
                            },
                        )
                        .await;
                        break;
                    }
                }
            }

            (false, ClientBody::Hello { protocol, client }) => {
                let device = agreed.device.clone();
                if protocol > PROTOCOL_VERSION {
                    let detail = format!(
                        "this cide speaks protocol {PROTOCOL_VERSION}; your app speaks {protocol} — update cide"
                    );
                    let _ = say(
                        &out_tx,
                        id_of,
                        ServerBody::Error {
                            kind: error_kind::PROTOCOL.to_owned(),
                            detail,
                        },
                    )
                    .await;
                    break;
                }
                if agreed.mode != Mode::Resume {
                    // A pairing connection says `Pair`, not `Hello`. Keeping the two roads apart
                    // is what stops there being a second way into the authenticated state.
                    let _ = say(
                        &out_tx,
                        id_of,
                        ServerBody::Error {
                            kind: error_kind::UNEXPECTED.to_owned(),
                            detail: "this connection was opened to pair, not to resume".to_owned(),
                        },
                    )
                    .await;
                    break;
                }

                inner.devices.note_seen(&device, &addr);
                writer_tag = format!("remote:{device}");
                identified = Some(device.clone());
                note(
                    &inner,
                    ServerEvent::Connected {
                        device: device.clone(),
                        addr: addr.clone(),
                    },
                );
                tracing::info!(%addr, %device, name = %client.name, "remote: a device connected");

                let instance = ask(&inner, |host| host.instance()).await;
                if say(
                    &out_tx,
                    id_of,
                    ServerBody::Welcome {
                        protocol: PROTOCOL_VERSION,
                        instance,
                        features: FEATURES.iter().map(|f| (*f).to_owned()).collect(),
                    },
                )
                .await
                .is_err()
                {
                    break;
                }
                // Flipped only once the welcome is away, so the fan-out cannot interleave a state
                // change ahead of the snapshot that establishes what it is about.
                authenticated.store(true, Ordering::Relaxed);
                if send_snapshot(&out_tx, &inner, None, Vec::new())
                    .await
                    .is_err()
                {
                    break;
                }
            }

            // --- anything else before a hello ------------------------------------------
            (false, _) => {
                let _ = say(
                    &out_tx,
                    id_of,
                    ServerBody::Error {
                        kind: error_kind::UNEXPECTED.to_owned(),
                        detail: "say hello first".to_owned(),
                    },
                )
                .await;
                break;
            }

            // --- authenticated ----------------------------------------------------------
            (true, ClientBody::Ping) => {
                if say(&out_tx, id_of, ServerBody::Pong).await.is_err() {
                    break;
                }
            }

            (true, ClientBody::Subscribe { projects }) => {
                // A replace. See `ClientBody::Subscribe`'s own doc for why it is never a delta.
                let (only, wanted) = {
                    let mut held = subscribed.lock();
                    *held = projects;
                    (lone(&held), held.clone())
                };
                if send_snapshot(&out_tx, &inner, only, wanted).await.is_err() {
                    break;
                }
            }

            (
                true,
                ClientBody::Input {
                    session,
                    key,
                    seq,
                    expect_screen,
                },
            ) => {
                if let Err(refusal) = guard_prompt(&inner, session, expect_screen.as_deref()).await
                {
                    if refused(&out_tx, id_of, Err(refusal)).await.is_err() {
                        break;
                    }
                    continue;
                }
                // The modes come from the same mirror the repaints do, read at the moment of the
                // keystroke: a device that had been told DECCKM was off four seconds ago must not
                // be the thing that decides what an arrow key is now.
                let Some(modes) =
                    ask(&inner, move |host| host.screen(session).map(|s| s.info)).await
                else {
                    let _ = say(
                        &out_tx,
                        id_of,
                        ServerBody::Error {
                            kind: error_kind::NO_SUCH.to_owned(),
                            detail: "that session is not running here".to_owned(),
                        },
                    )
                    .await;
                    continue;
                };
                let bytes = crate::keys::encode(&key, &modes);
                if bytes.is_empty() {
                    // A key this cide has no encoding for. Answered rather than silently dropped,
                    // and never guessed at: the alternative to an unknown key is a wrong key, and
                    // a wrong key in a terminal is an action.
                    let _ = say(
                        &out_tx,
                        id_of,
                        ServerBody::Error {
                            kind: error_kind::UNKNOWN_FRAME.to_owned(),
                            detail: "this cide has no encoding for that key".to_owned(),
                        },
                    )
                    .await;
                    continue;
                }
                if let Err(why) = write_into(&inner, session, bytes, &writer_tag, id, seq).await
                    && say(
                        &out_tx,
                        id_of,
                        ServerBody::Error {
                            kind: error_kind::REFUSED.to_owned(),
                            detail: why,
                        },
                    )
                    .await
                    .is_err()
                {
                    break;
                }
            }

            (
                true,
                ClientBody::Paste {
                    session,
                    text,
                    seq,
                    expect_screen,
                },
            ) => {
                if let Err(refusal) = guard_prompt(&inner, session, expect_screen.as_deref()).await
                {
                    if refused(&out_tx, id_of, Err(refusal)).await.is_err() {
                        break;
                    }
                    continue;
                }
                let Some(modes) =
                    ask(&inner, move |host| host.screen(session).map(|s| s.info)).await
                else {
                    let _ = say(
                        &out_tx,
                        id_of,
                        ServerBody::Error {
                            kind: error_kind::NO_SUCH.to_owned(),
                            detail: "that session is not running here".to_owned(),
                        },
                    )
                    .await;
                    continue;
                };
                let bytes = crate::keys::paste(&text, &modes);
                if let Err(why) = write_into(&inner, session, bytes, &writer_tag, id, seq).await
                    && say(
                        &out_tx,
                        id_of,
                        ServerBody::Error {
                            kind: error_kind::REFUSED.to_owned(),
                            detail: why,
                        },
                    )
                    .await
                    .is_err()
                {
                    break;
                }
            }

            // Scrolling the *program*, for a session the terminal keeps no history for.
            //
            // Shaped exactly like `Paste`: the modes come from the mirror at the moment of the
            // gesture, never from the device, because whether these bytes are input at all is
            // the child's question and not the phone's. An empty encoding is a refusal and is
            // reported as one — silence here would be a scroll gesture that does nothing, on a
            // screen that cannot show why.
            (
                true,
                ClientBody::Scroll {
                    session,
                    lines,
                    seq,
                },
            ) => {
                let Some(modes) =
                    ask(&inner, move |host| host.screen(session).map(|s| s.info)).await
                else {
                    let _ = say(
                        &out_tx,
                        id_of,
                        ServerBody::Error {
                            kind: error_kind::NO_SUCH.to_owned(),
                            detail: "that session is not running here".to_owned(),
                        },
                    )
                    .await;
                    continue;
                };
                let bytes = crate::keys::wheel(lines, &modes);
                if bytes.is_empty() {
                    let _ = say(
                        &out_tx,
                        id_of,
                        ServerBody::Error {
                            kind: error_kind::REFUSED.to_owned(),
                            detail: "that program is not asking for the mouse, so a scroll would \
                                     reach it as typed characters"
                                .to_owned(),
                        },
                    )
                    .await;
                    continue;
                }
                if let Err(why) = write_into(&inner, session, bytes, &writer_tag, id, seq).await
                    && say(
                        &out_tx,
                        id_of,
                        ServerBody::Error {
                            kind: error_kind::REFUSED.to_owned(),
                            detail: why,
                        },
                    )
                    .await
                    .is_err()
                {
                    break;
                }
            }

            (
                true,
                ClientBody::RunStop {
                    project,
                    run,
                    reason,
                    force,
                },
            ) => {
                let done = ask(&inner, move |host| {
                    host.run_stop(project, run, reason, force)
                })
                .await;
                if refused(&out_tx, id_of, done).await.is_err() {
                    break;
                }
            }

            (true, ClientBody::RunPause { project, run }) => {
                let done = ask(&inner, move |host| host.run_pause(project, run)).await;
                if refused(&out_tx, id_of, done).await.is_err() {
                    break;
                }
            }

            (true, ClientBody::RunResume { project, run }) => {
                let done = ask(&inner, move |host| host.run_resume(project, run)).await;
                if refused(&out_tx, id_of, done).await.is_err() {
                    break;
                }
            }

            (true, ClientBody::Dispatch { request }) => {
                match ask(&inner, move |host| host.dispatch(request)).await {
                    Ok(run) => {
                        if say(&out_tx, id_of, ServerBody::Dispatched { run })
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(detail) => {
                        if refused(&out_tx, id_of, Err(detail)).await.is_err() {
                            break;
                        }
                    }
                }
            }

            (true, ClientBody::TaskNew { task }) => {
                let done = ask(&inner, move |host| host.task_new(task)).await;
                if refused(&out_tx, id_of, done).await.is_err() {
                    break;
                }
            }

            (
                true,
                ClientBody::TaskEdit {
                    project,
                    task,
                    edit,
                },
            ) => {
                let done = ask(&inner, move |host| host.task_edit(project, task, edit)).await;
                if refused(&out_tx, id_of, done).await.is_err() {
                    break;
                }
            }

            (true, ClientBody::TaskGet { project, task }) => {
                // Answered, never pushed. A device holds rows and asks for contents only when
                // somebody opens one, which is why the board carries none.
                let wanted = task.clone();
                let found = ask(&inner, move |host| host.task(project, wanted))
                    .await
                    .map(Box::new);
                if say(
                    &out_tx,
                    id_of,
                    ServerBody::Task {
                        project,
                        id: task,
                        task: found,
                    },
                )
                .await
                .is_err()
                {
                    break;
                }
            }

            (
                true,
                ClientBody::AnswerPrompt {
                    session,
                    option,
                    expect_screen,
                },
            ) => {
                let done = ask(&inner, move |host| {
                    host.answer_prompt(session, option, &expect_screen)
                })
                .await;
                if refused(&out_tx, id_of, done).await.is_err() {
                    break;
                }
            }

            (true, ClientBody::Acknowledge { session }) => {
                let done = ask(&inner, move |host| host.acknowledge(session)).await;
                if refused(&out_tx, id_of, done).await.is_err() {
                    break;
                }
            }

            (true, ClientBody::WatchScreen { session }) => {
                // A fresh watch owes a whole grid, which the next tick supplies. Inserting rather
                // than replacing an existing one is deliberate: a device that asks twice has not
                // forgotten what it holds, and resending the screen would be a visible flicker
                // for nothing.
                watches
                    .lock()
                    .entry(session)
                    .or_insert_with(crate::screen::Watch::new);
            }

            (true, ClientBody::UnwatchScreen { session }) => {
                watches.lock().remove(&session);
            }

            (
                true,
                ClientBody::ScrollbackPage {
                    session,
                    from_top,
                    rows,
                },
            ) => {
                let page = ask(&inner, move |host| host.scrollback(session, from_top, rows)).await;
                let body = match page {
                    Some(page) => ServerBody::Scrollback { session, page },
                    None => ServerBody::Error {
                        kind: error_kind::NO_SUCH.to_owned(),
                        detail: "that session is not running here".to_owned(),
                    },
                };
                if say(&out_tx, id_of, body).await.is_err() {
                    break;
                }
            }

            (true, ClientBody::Hello { .. } | ClientBody::Pair { .. }) => {
                let _ = say(
                    &out_tx,
                    id_of,
                    ServerBody::Error {
                        kind: error_kind::UNEXPECTED.to_owned(),
                        detail: "this connection has already said hello".to_owned(),
                    },
                )
                .await;
            }
        }
    }

    inner.forget_conn(id);
    // Cleared only if this connection is still the one on screen. A later device replaced it if
    // one arrived, and taking the slot down on *any* pairing socket closing would blank the
    // number belonging to a connection that is still waiting to be compared against.
    {
        let mut pairing = inner.pairing.lock();
        if pairing.as_ref().is_some_and(|(owner, _)| *owner == id) {
            *pairing = None;
            drop(pairing);
            note(&inner, ServerEvent::PairingEnded);
        }
    }
    // Aborted rather than left to notice: it holds a sender, so the writer's queue would never
    // close and the writer would never finish.
    watcher.abort();
    drop(out_tx);
    let _ = writer.await;
    if let Some(device) = identified {
        note(&inner, ServerEvent::Disconnected { device });
    }
}

/// Refuse free text at a permission prompt the device did not say it could see.
///
/// A prompt accepts typed redirection — that is what its third option is for — so refusing text
/// outright would take a real capability away. What must not happen is a stray Enter from a soft
/// keyboard landing on a prompt that arrived while a thumb was moving, because at a selection
/// list a carriage return **is an answer**. The device can see the screen, so it is asked to say
/// which prompt it is answering, and a mismatch is refused.
///
/// No prompt on screen, no obligation: this costs a host call only while a session is actually
/// asking something.
async fn guard_prompt(
    inner: &Arc<Inner>,
    session: cide_ipc::SessionId,
    expect_screen: Option<&str>,
) -> Result<(), String> {
    let Some(prompt) = ask(inner, move |host| host.prompt(session)).await else {
        return Ok(());
    };
    match expect_screen {
        None => Err(
            "that session is asking a permission question — send the prompt's digest with your \
             keystroke, or answer it by picking an option"
                .to_owned(),
        ),
        Some(offered) if offered == prompt.digest => Ok(()),
        Some(_) => Err("that prompt has changed since you were shown it".to_owned()),
    }
}

/// The envelope's `id`, recovered from a frame whose **body** did not parse.
///
/// The refusal above is only worth sending if it reaches the request that asked for it. A device
/// keys its outstanding questions by id, so an `Error` carrying none is delivered as an
/// unsolicited event and the asker waits out its own timeout instead — fifteen seconds in
/// `cide-mobile`, ending in *"cide did not answer that in time"* about a cide that answered
/// immediately and said exactly what was wrong. Which is the worst shape available for this
/// case, because the whole point of answering an unreadable frame is that a device built against
/// a newer cide learns something.
///
/// The id survives a body that does not, because the envelope is `{ id, body }` and only `body`
/// carries the vocabulary: `ClientFrame`'s own derive refuses the pair as a unit, so the id is
/// read again here rather than out of a partial parse that does not exist. Anything that is not
/// an object, or an object whose `id` is missing or not a `u32`, answers `None` — the frame is
/// already unreadable and a guessed id would resolve *somebody else's* question.
fn envelope_id(plaintext: &[u8]) -> Option<u32> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        #[serde(default)]
        id: Option<u32>,
    }
    serde_json::from_slice::<Envelope>(plaintext).ok()?.id
}

/// Turn a host's refusal into a frame, and say nothing at all on success.
///
/// Silence on success is deliberate and is the same choice every write frame here makes: a device
/// that needs to know what happened is already subscribed to the board, and an acknowledgement
/// frame per gesture would be a second, weaker source of truth about the same thing.
async fn refused(
    out: &mpsc::Sender<ServerFrame>,
    id: Option<u32>,
    outcome: Result<(), String>,
) -> Result<(), ()> {
    let Err(detail) = outcome else {
        return Ok(());
    };
    say(
        out,
        id,
        ServerBody::Error {
            kind: error_kind::REFUSED.to_owned(),
            detail,
        },
    )
    .await
}

/// Hand bytes to a session, through cide's own at-least-once watermark.
///
/// `epoch` is this connection's id rather than the device's, and that is what makes a reconnect
/// correct: the watermark is per `(session, writer)` and a new epoch resets it, so a device whose
/// socket dropped and whose sequence counter restarted does not have its first keystrokes
/// discarded as duplicates of the last ones it sent.
async fn write_into(
    inner: &Arc<Inner>,
    session: cide_ipc::SessionId,
    bytes: Vec<u8>,
    writer: &str,
    epoch: u64,
    seq: u64,
) -> Result<(), String> {
    let writer = writer.to_owned();
    let epoch = epoch.to_string();
    ask(inner, move |host| {
        host.write(session, bytes, &writer, &epoch, seq)
    })
    .await
}

/// Read the grids this connection is watching, and send what moved.
///
/// One task per connection rather than per watched session: a phone watches one terminal at a
/// time in practice, and a task per session would be a task per tab somebody left open.
///
/// **The frames are queued with `send().await`, not `try_send`.** That is the opposite of the
/// fan-out's rule and it is right for the opposite reason: waiting here throttles the *polling*
/// rather than dropping a repaint, so a device that is behind simply gets read less often. The
/// thing being made to wait is this timer, which nothing else depends on.
async fn watch_loop(inner: Arc<Inner>, out: mpsc::Sender<ServerFrame>, watches: Watches) {
    let mut ticks: u64 = 0;
    loop {
        tokio::time::sleep(SCREEN_TICK).await;
        ticks += 1;

        let sessions: Vec<cide_ipc::SessionId> = watches.lock().keys().copied().collect();
        for session in sessions {
            let now = std::time::Instant::now();
            let due = {
                let held = watches.lock();
                held.get(&session).is_some_and(|watch| {
                    watch.idle_for(now) < SCREEN_IDLE_AFTER || ticks.is_multiple_of(IDLE_EVERY)
                })
            };
            if !due {
                continue;
            }

            let Some(capture) = ask(&inner, move |host| host.screen(session)).await else {
                // The child is gone. Said out loud, because on a phone a screen that has stopped
                // changing and a screen of a process that exited an hour ago are one picture.
                watches.lock().remove(&session);
                if say(&out, None, ServerBody::ScreenGone { session })
                    .await
                    .is_err()
                {
                    return;
                }
                continue;
            };

            let update = {
                let mut held = watches.lock();
                held.get_mut(&session)
                    .and_then(|watch| watch.diff(session, capture))
            };
            if let Some(update) = update
                && say(&out, None, ServerBody::Screen { update })
                    .await
                    .is_err()
            {
                return;
            }

            // The prompt after the rows, so a device drawing a card over the terminal has the
            // terminal underneath it first. Read every due tick and sent only when it *changed* —
            // `prompt_news` keys on the digest, so re-sending the same question would redraw a
            // card under somebody's thumb.
            let prompt = ask(&inner, move |host| host.prompt(session)).await;
            let news = {
                let mut held = watches.lock();
                held.get_mut(&session)
                    .and_then(|watch| watch.prompt_news(session, prompt))
            };
            let body = match news {
                None => continue,
                Some(crate::screen::PromptNews::Asking(prompt)) => ServerBody::Prompt {
                    session,
                    prompt: *prompt,
                },
                Some(crate::screen::PromptNews::Gone(session)) => {
                    ServerBody::PromptGone { session }
                }
            };
            if say(&out, None, body).await.is_err() {
                return;
            }
        }
    }
}

/// The one task that writes to a socket.
///
/// Everything that wants to say something to a device queues a frame; this drains the queue. Two
/// tasks writing to one `SplitSink` would need a lock held across a network write, which is the
/// thing the queue exists to avoid.
async fn write_loop(
    mut sink: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        Message,
    >,
    mut queue: mpsc::Receiver<ServerFrame>,
    desynced: Arc<AtomicBool>,
    mut sealer: Sealer,
) {
    while let Some(frame) = queue.recv().await {
        // A drop happened while this socket was behind. Say so *before* the next frame, so the
        // device re-reads rather than believing a gap it cannot see.
        if desynced.swap(false, Ordering::Relaxed)
            && send_frame(
                &mut sink,
                &mut sealer,
                ServerFrame {
                    id: None,
                    body: ServerBody::Desync {
                        why: "this connection fell behind and missed some news".to_owned(),
                    },
                },
            )
            .await
            .is_err()
        {
            return;
        }
        if send_frame(&mut sink, &mut sealer, frame).await.is_err() {
            return;
        }
    }
    let _ = sink.close().await;
}

async fn send_frame(
    sink: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
        Message,
    >,
    sealer: &mut Sealer,
    frame: ServerFrame,
) -> Result<(), ()> {
    let json = match serde_json::to_vec(&frame) {
        Ok(json) => json,
        Err(error) => {
            tracing::error!(%error, "remote: a frame would not serialise");
            return Err(());
        }
    };
    let boxed = sealer.seal(&json).map_err(|error| {
        tracing::error!(%error, "remote: a frame would not seal");
    })?;
    sink.send(Message::binary(boxed)).await.map_err(|_| ())
}

/// Projects, sessions and the awaiting set, in that order.
///
/// One function so the snapshot a connection gets at hello and the one it gets after a
/// `Subscribe` cannot drift apart — a device that painted a different list depending on which of
/// the two it received last would be a bug nobody could reproduce on purpose.
async fn send_snapshot(
    out: &mpsc::Sender<ServerFrame>,
    inner: &Arc<Inner>,
    only: Option<cide_ipc::ProjectId>,
    projects: Vec<cide_ipc::ProjectId>,
) -> Result<(), ()> {
    let (rev, open) = ask(inner, |host| host.projects()).await;
    say(
        out,
        None,
        ServerBody::Projects {
            rev,
            projects: open,
        },
    )
    .await?;
    let sessions = ask(inner, move |host| host.sessions(only)).await;
    say(out, None, ServerBody::Sessions { sessions }).await?;
    let entries = ask(inner, |host| host.awaiting()).await;
    say(out, None, ServerBody::Awaiting { entries }).await?;
    // Runs, roles and tasks, for **every** project this device is interested in.
    //
    // Not just for a lone subscription. These three reads are per project, and an earlier cut
    // sent them only when exactly one was subscribed — which meant a device showing every
    // project's consoles could not show any project's agents, and the two screens disagreed
    // about what the device was looking at. A device filters by project locally, exactly as it
    // already does for sessions.
    for project in interests(inner, &projects).await {
        send_project(out, inner, project).await?;
    }
    Ok(())
}

/// Which projects a subscription covers.
///
/// An empty subscription means *everything*, which is `sessions(None)`'s rule and has to stay
/// the same rule here — two spellings of "all" is how one screen ends up showing a project the
/// other does not.
async fn interests(
    inner: &Arc<Inner>,
    subscribed: &[cide_ipc::ProjectId],
) -> Vec<cide_ipc::ProjectId> {
    if !subscribed.is_empty() {
        return subscribed.to_vec();
    }
    ask(inner, |host| host.projects())
        .await
        .1
        .into_iter()
        .map(|project| project.id)
        .collect()
}

/// The three per-project reads, together.
///
/// One function because they are always wanted together and always for the same project: the
/// device's Agents screen needs the roster *and* the runs to draw a row, and splitting them
/// across two call sites is how one of them stops being sent.
async fn send_project(
    out: &mpsc::Sender<ServerFrame>,
    inner: &Arc<Inner>,
    project: cide_ipc::ProjectId,
) -> Result<(), ()> {
    let runs = ask(inner, move |host| host.runs(project)).await;
    say(out, None, ServerBody::Runs { project, runs }).await?;
    let roster = ask(inner, move |host| host.roster(project)).await;
    say(
        out,
        None,
        ServerBody::Roster {
            project,
            agents: roster.agents,
            dispatching: roster.dispatching,
        },
    )
    .await?;
    let tasks = ask(inner, move |host| host.board(project)).await;
    say(out, None, ServerBody::Board { project, tasks }).await
}

/// Ask the host something, off the runtime.
///
/// Every host method takes a lock the application's own threads hold, so calling one inline would
/// park a runtime worker behind the hook-apply thread. See [`crate::host`]'s header.
async fn ask<T, F>(inner: &Arc<Inner>, f: F) -> T
where
    T: Send + 'static,
    F: FnOnce(&dyn RemoteHost) -> T + Send + 'static,
{
    let host = Arc::clone(&inner.host);
    match tokio::task::spawn_blocking(move || f(host.as_ref())).await {
        Ok(answer) => answer,
        // The only way this fails is a panic inside the host, which has already been reported by
        // the panic hook. Re-panicking here would take the whole runtime, and with it every other
        // device's connection, over one bad read.
        Err(error) => {
            tracing::error!(%error, "remote: a host call panicked");
            std::process::abort()
        }
    }
}

/// Queue one frame towards this connection's device.
///
/// `send().await` rather than `try_send`, and that asymmetry with [`Inner::push`] is deliberate:
/// this is called from the connection's *own* task, where waiting for room is correct and where
/// dropping a reply to a question the device asked would be a hang on the other end. The fan-out
/// is the opposite case and must never wait.
async fn say(out: &mpsc::Sender<ServerFrame>, id: Option<u32>, body: ServerBody) -> Result<(), ()> {
    out.send(ServerFrame { id, body }).await.map_err(|_| ())
}

fn note(inner: &Arc<Inner>, event: ServerEvent) {
    let tx = inner.events.lock().clone();
    // `try_send`, never `send`: the panel is a courtesy and a connection must not wait on it.
    if let Err(error) = tx.try_send(event) {
        tracing::debug!(%error, "remote: an event reached nobody");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cide_ipc::remote::{AwaitingEntry, InstanceInfo, RemoteProject, RemoteSession};
    use cide_ipc::{PaneId, PaneKind, PaneRole, ProjectId, SessionId, SessionState};
    use tokio_tungstenite::MaybeTlsStream;

    /// One call to [`RemoteHost::write`], as the fake recorded it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Wrote {
        session: SessionId,
        bytes: Vec<u8>,
        writer: String,
        seq: u64,
    }

    /// cide, as far as this crate is concerned.
    ///
    /// The whole reason [`RemoteHost`] is a trait. Every test below drives the real server over a
    /// real socket with the real protocol, and none of them needs a workspace, a PTY, a `claude`
    /// on PATH or a display — which is what makes them the crate's *first* end-to-end coverage
    /// rather than a suite of `#[ignore]`d ones.
    ///
    /// Assertions are made against what the host **recorded**, not only against what the client
    /// was told. "The answer was refused" proved by an error frame is a weaker claim than the
    /// same thing proved by the host never having been asked.
    struct FakeHost {
        projects: Vec<RemoteProject>,
        sessions: Vec<RemoteSession>,
        awaiting: Vec<AwaitingEntry>,
        asked: Mutex<Vec<Option<ProjectId>>>,
        projects_asked: AtomicU64,
        screens: Mutex<std::collections::HashMap<SessionId, cide_ipc::screen::ScreenCapture>>,
        screens_read: AtomicU64,
        written: Mutex<Vec<Wrote>>,
        acknowledged: Mutex<Vec<SessionId>>,
        run_calls: Mutex<Vec<String>>,
        prompts: Mutex<std::collections::HashMap<SessionId, cide_ipc::remote::PermissionPrompt>>,
        answered: Mutex<Vec<(SessionId, u8)>>,
        task_calls: Mutex<Vec<String>>,
        runs: Mutex<Vec<cide_ipc::AgentRun>>,
        agents: Mutex<Vec<cide_ipc::remote::RemoteAgent>>,
        tasks: Mutex<Vec<cide_ipc::TaskRow>>,
        dispatching: Mutex<bool>,
        details: Mutex<std::collections::HashMap<String, cide_ipc::TaskDetail>>,
        refuse_writes: Mutex<Option<String>>,
        scrollback: Mutex<std::collections::HashMap<SessionId, Vec<cide_ipc::screen::ScreenLine>>>,
    }

    impl FakeHost {
        fn with_two_projects() -> (Arc<Self>, ProjectId, ProjectId) {
            let first = ProjectId::new();
            let second = ProjectId::new();
            let host = Arc::new(Self {
                projects: vec![project(first, "cide"), project(second, "other")],
                sessions: vec![session(first), session(second), session(second)],
                awaiting: vec![AwaitingEntry {
                    session: SessionId::new(),
                    since_unix_ms: 1_700_000_000_000,
                }],
                asked: Mutex::new(Vec::new()),
                projects_asked: AtomicU64::new(0),
                screens: Mutex::new(std::collections::HashMap::new()),
                screens_read: AtomicU64::new(0),
                written: Mutex::new(Vec::new()),
                acknowledged: Mutex::new(Vec::new()),
                run_calls: Mutex::new(Vec::new()),
                prompts: Mutex::new(std::collections::HashMap::new()),
                answered: Mutex::new(Vec::new()),
                task_calls: Mutex::new(Vec::new()),
                refuse_writes: Mutex::new(None),
                scrollback: Mutex::new(std::collections::HashMap::new()),
                runs: Mutex::new(Vec::new()),
                agents: Mutex::new(Vec::new()),
                tasks: Mutex::new(Vec::new()),
                dispatching: Mutex::new(true),
                details: Mutex::new(std::collections::HashMap::new()),
            });
            (host, first, second)
        }
    }

    fn project(id: ProjectId, name: &str) -> RemoteProject {
        RemoteProject {
            id,
            name: name.to_owned(),
            display_path: format!("~/work/{name}"),
            dot: "var(--accent)".to_owned(),
        }
    }

    fn session(project: ProjectId) -> RemoteSession {
        RemoteSession {
            session: SessionId::new(),
            project,
            pane: PaneId::new(),
            tab: None,
            tab_title: None,
            title: "claude".to_owned(),
            kind: PaneKind::Claude,
            role: PaneRole::Primary,
            state: SessionState::Idle,
            awaiting: false,
            run: None,
            agent: None,
            task: None,
        }
    }

    impl RemoteHost for FakeHost {
        fn instance(&self) -> InstanceInfo {
            InstanceInfo {
                id: "i-test".to_owned(),
                name: "[DEV] testbox".to_owned(),
                profile: Some("test".to_owned()),
                version: "0.0.0-test".to_owned(),
            }
        }

        fn projects(&self) -> (u64, Vec<RemoteProject>) {
            self.projects_asked.fetch_add(1, Ordering::Relaxed);
            (7, self.projects.clone())
        }

        fn runs(&self, project: ProjectId) -> Vec<cide_ipc::AgentRun> {
            self.runs
                .lock()
                .iter()
                .filter(|run| run.project == project)
                .cloned()
                .collect()
        }

        fn roster(&self, _project: ProjectId) -> cide_ipc::remote::RemoteRoster {
            cide_ipc::remote::RemoteRoster {
                agents: self.agents.lock().clone(),
                dispatching: *self.dispatching.lock(),
            }
        }

        fn board(&self, _project: ProjectId) -> Vec<cide_ipc::TaskRow> {
            self.tasks.lock().clone()
        }

        fn task(
            &self,
            _project: ProjectId,
            task: cide_ipc::TaskId,
        ) -> Option<cide_ipc::TaskDetail> {
            self.details.lock().get(&task.to_string()).cloned()
        }

        fn sessions(&self, only: Option<ProjectId>) -> Vec<RemoteSession> {
            self.asked.lock().push(only);
            self.sessions
                .iter()
                .filter(|s| only.is_none_or(|wanted| wanted == s.project))
                .cloned()
                .collect()
        }

        fn awaiting(&self) -> Vec<AwaitingEntry> {
            self.awaiting.clone()
        }

        fn screen(&self, session: SessionId) -> Option<cide_ipc::screen::ScreenCapture> {
            self.screens_read.fetch_add(1, Ordering::Relaxed);
            self.screens.lock().get(&session).cloned()
        }

        fn write(
            &self,
            session: SessionId,
            bytes: Vec<u8>,
            writer: &str,
            _epoch: &str,
            seq: u64,
        ) -> Result<(), String> {
            if let Some(why) = self.refuse_writes.lock().clone() {
                return Err(why);
            }
            self.written.lock().push(Wrote {
                session,
                bytes,
                writer: writer.to_owned(),
                seq,
            });
            Ok(())
        }

        fn acknowledge(&self, session: SessionId) -> Result<(), String> {
            self.acknowledged.lock().push(session);
            Ok(())
        }

        fn prompt(&self, session: SessionId) -> Option<cide_ipc::remote::PermissionPrompt> {
            self.prompts.lock().get(&session).cloned()
        }

        fn answer_prompt(
            &self,
            session: SessionId,
            option: u8,
            expect_screen: &str,
        ) -> Result<(), String> {
            // The fake keeps the host's own rule, because the tests below are about the server
            // asking at all — the rule's *own* coverage is `cide-app`'s.
            let held = self.prompts.lock();
            let Some(prompt) = held.get(&session) else {
                return Err("that session is not asking a permission question".to_owned());
            };
            if prompt.digest != expect_screen {
                return Err("that prompt has changed since you were shown it".to_owned());
            }
            drop(held);
            self.answered.lock().push((session, option));
            Ok(())
        }

        fn dispatch(&self, request: cide_ipc::DispatchRequest) -> Result<cide_ipc::RunId, String> {
            self.task_calls
                .lock()
                .push(format!("dispatch {}", request.agent.0));
            Ok(cide_ipc::RunId::new())
        }

        fn task_new(&self, task: cide_ipc::TaskNew) -> Result<(), String> {
            self.task_calls.lock().push(format!("new {}", task.title));
            Ok(())
        }

        fn task_edit(
            &self,
            _project: ProjectId,
            task: cide_ipc::TaskId,
            _edit: cide_ipc::TaskEdit,
        ) -> Result<(), String> {
            self.task_calls.lock().push(format!("edit {}", task.0));
            Ok(())
        }

        fn run_stop(
            &self,
            project: ProjectId,
            run: cide_ipc::RunId,
            reason: Option<String>,
            force: bool,
        ) -> Result<(), String> {
            if project != self.projects[0].id {
                return Err("no such project".to_owned());
            }
            self.run_calls
                .lock()
                .push(format!("stop {run} force={force} reason={reason:?}"));
            Ok(())
        }

        fn run_pause(
            &self,
            _project: ProjectId,
            run: Option<cide_ipc::RunId>,
        ) -> Result<(), String> {
            self.run_calls.lock().push(format!("pause {run:?}"));
            Ok(())
        }

        fn run_resume(
            &self,
            _project: ProjectId,
            run: Option<cide_ipc::RunId>,
        ) -> Result<(), String> {
            self.run_calls.lock().push(format!("resume {run:?}"));
            Ok(())
        }

        fn scrollback(
            &self,
            session: SessionId,
            from_top: u32,
            rows: u16,
        ) -> Option<cide_ipc::screen::ScrollbackCapture> {
            let held = self.scrollback.lock();
            let lines = held.get(&session)?;
            let first = (from_top as usize).min(lines.len());
            let last = (first + rows as usize).min(lines.len());
            Some(cide_ipc::screen::ScrollbackCapture {
                from_top: first as u32,
                depth: lines.len() as u32,
                lines: lines[first..last].to_vec(),
            })
        }
    }

    /// A device, as far as these tests are concerned: a socket and both halves of the channel.
    ///
    /// Everything below goes through the **real** sealed transport. That is not thoroughness for
    /// its own sake — it means the handshake, the key derivation, the nonce discipline and the
    /// counter check are exercised by every test in this file rather than by the handful that
    /// are about them.
    struct Client {
        ws: tokio_tungstenite::WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>,
        opener: crate::seal::Opener,
        sealer: crate::seal::Sealer,
    }

    struct Harness {
        server: RemoteServer,
        devices: Arc<DeviceStore>,
        host: Arc<FakeHost>,
        statik: Arc<StaticKey>,
        url: String,
    }

    async fn harness() -> (Harness, ProjectId, ProjectId) {
        let (host, first, second) = FakeHost::with_two_projects();
        let devices = Arc::new(DeviceStore::ephemeral());
        let statik = Arc::new(StaticKey::ephemeral());
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.expect("binds");
        let server = RemoteServer::attach(
            listener,
            Arc::clone(&host) as Arc<dyn RemoteHost>,
            Arc::clone(&devices),
            Arc::clone(&statik),
        )
        .expect("starts");
        let url = format!("ws://127.0.0.1:{}", server.port());
        (
            Harness {
                server,
                devices,
                host,
                statik,
                url,
            },
            first,
            second,
        )
    }

    impl Harness {
        fn run_calls(&self) -> Vec<String> {
            self.host.run_calls.lock().clone()
        }
    }

    /// Open a **pairing** connection: a device with no key yet.
    ///
    /// This is the only unauthenticated state that exists now. A socket that shakes hands as a
    /// device cide does not know is refused at the handshake, because without a key there is no
    /// channel to refuse it *in* — which is [`an_unknown_device_is_told_so_and_reads_nothing`].
    #[tokio::test]
    async fn a_pairing_connection_is_published_for_a_person_to_compare_and_a_resuming_one_is_not() {
        let (h, _, _) = harness().await;

        // Nothing is pairing, so there is nothing to compare against. A number drawn here would
        // be a number for no connection at all.
        assert_eq!(h.server.pairing_attempt(), None);

        let pairing = connect(&h).await;
        let shown = wait_for_attempt(&h)
            .await
            .expect("a pairing device is published");
        // Six digits, grouped once, here. Two screens that spaced it differently would turn a
        // match into a mismatch, which is the failure that makes people stop checking.
        assert_eq!(shown.sas.len(), 7, "{}", shown.sas);
        assert_eq!(shown.sas.as_bytes()[3], b' ');
        assert!(shown.addr.starts_with("127.0.0.1:"), "{}", shown.addr);

        drop(pairing);
        // And it goes when the connection does, or the panel goes on offering a number for a
        // socket that is closed — the one value worse than none, because it is acceptable.
        for _ in 0..50 {
            if h.server.pairing_attempt().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(h.server.pairing_attempt(), None);

        // A paired device resuming has a pinned key and a token, so there is nothing for a
        // person to adjudicate. Publishing one here would train somebody to wave through the
        // screen that matters by showing it on every reconnect.
        let (device, key) = pair(&h).await;
        // Pairing itself publishes one, so it is cleared before the claim below is made.
        for _ in 0..50 {
            if h.server.pairing_attempt().is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let _resumed = resumed(&h, &device, &key).await;
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert_eq!(h.server.pairing_attempt(), None);
    }

    #[tokio::test]
    async fn a_pairing_socket_outlives_the_wait_a_person_takes_to_compare_the_digits() {
        // Found by pairing by hand and taking forty seconds over it. The socket was dropped at
        // twenty, and the app reported *the connection failed* — a network problem, which it
        // was not, so the advice it implies (try again, move closer to the router) is wrong.
        //
        // The two constants are asserted against each other rather than against literals: the
        // claim is not "it is 135 seconds", it is "a socket may wait for as long as its code
        // could still be redeemed", and that stays true if either number moves.
        assert!(
            PAIRING_TIMEOUT > crate::devices::CODE_TTL,
            "a pairing socket must outlive the code it is waiting to redeem"
        );
        assert!(
            PAIRING_TIMEOUT > HANDSHAKE_TIMEOUT,
            "a pairing socket waits on a person, not on a packet"
        );

        // And the socket really does stay open past the plain handshake allowance. Driven for a
        // real interval rather than asserted on a constant, because the arm that chooses
        // between the two is the thing that can be got wrong.
        let (h, _, _) = harness().await;
        let mut pairing = connect(&h).await;
        assert!(wait_for_attempt(&h).await.is_some());

        // On tokio's clock rather than the wall's: this waits out a twenty-second timeout, and a
        // test that actually slept for it would be twenty times the rest of this file put
        // together. `pause` stops the clock, `advance` moves it, and the timer the connection
        // task is parked on fires as though the interval had passed.
        tokio::time::pause();
        tokio::time::advance(HANDSHAKE_TIMEOUT + Duration::from_secs(2)).await;
        tokio::time::resume();
        // Let the connection task run on the resumed clock before anything is asked of it.
        tokio::task::yield_now().await;

        // Still alive, and still able to redeem — which is the whole claim.
        let code = h.devices.begin_pairing().expect("opens a pairing window");
        say(
            &mut pairing,
            Some(1),
            ClientBody::Pair {
                code: crate::devices::grouped(&code),
                client: client_info(),
            },
        )
        .await;
        assert!(
            matches!(heard(&mut pairing).await, Some(ServerBody::Paired { .. })),
            "a pairing socket that waited must still be able to redeem"
        );
    }

    /// The attempt, once the server has recorded it. Polled because the connection task records
    /// it after `shake` has returned on the client's side.
    async fn wait_for_attempt(h: &Harness) -> Option<cide_ipc::remote::PairingAttempt> {
        for _ in 0..50 {
            if let Some(attempt) = h.server.pairing_attempt() {
                return Some(attempt);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        None
    }

    #[tokio::test]
    async fn a_stopped_server_gives_its_port_back_immediately() {
        // The bug this exists for, reported from a phone: open a pairing window, fail to
        // connect, press Cancel, press "Pair a device…" again — *port already in use*, for ever,
        // and switching the whole feature off did not free it either.
        //
        // The stop was `Arc::try_unwrap(server)` with a `shutdown` that consumed `Self`, so it
        // only ran when nothing else held a clone — and the event pump always does. The losing
        // arm dropped a refcount, the accept loop kept the `TcpListener`, and cide spent the
        // rest of the session refusing itself. With a **configured** port that is terminal,
        // because a port somebody typed is a promise to a device and is never slid off.
        //
        // Re-binding the same port is the whole assertion, and it has to be a *fixed* port:
        // asking for :0 twice would pass against a server that never stopped at all.
        let (host, _, _) = FakeHost::with_two_projects();
        let devices = Arc::new(DeviceStore::ephemeral());
        let statik = Arc::new(StaticKey::ephemeral());

        // Borrow a port from the OS, then let it go, so the number below is one nothing else on
        // the machine is using — the same trick as asking for :0, without keeping the socket.
        let port = {
            let scout = TcpListener::bind(("127.0.0.1", 0)).await.expect("binds");
            scout.local_addr().expect("has an address").port()
        };
        let addr: SocketAddr = ([127, 0, 0, 1], port).into();

        for attempt in 0..3 {
            let server = RemoteServer::bind(
                addr,
                Arc::clone(&host) as Arc<dyn RemoteHost>,
                Arc::clone(&devices),
                Arc::clone(&statik),
            )
            .await
            .unwrap_or_else(|e| panic!("attempt {attempt} could not bind {addr}: {e}"));

            // Held through an `Arc`, exactly as the application holds it — a second clone
            // standing in for the event pump, which is what made `try_unwrap` fail. A stop that
            // works only when nobody else is looking is the bug, so the test always looks.
            let server = Arc::new(server);
            let pretend_pump = Arc::clone(&server);

            server.shutdown().await;
            drop(pretend_pump);
        }
    }

    #[tokio::test]
    async fn subscribing_to_a_project_serves_its_runs_roster_and_board() {
        // The gap this closes: `run_stop`, `run_pause` and `run_resume` were on the wire from
        // M72 while nothing could *list* a run, so a device could pause a run it had no way to
        // see. A control for a thing that cannot be shown is a guess with a button on it.
        let (h, first, second) = harness().await;
        h.host.runs.lock().push(cide_ipc::AgentRun {
            run: uuid::Uuid::new_v4().into(),
            agent: "reviewer".to_owned().into(),
            agent_label: "Reviewer".to_owned(),
            harness: cide_ipc::Harness::Claude,
            project: first,
            session: None,
            state: cide_ipc::RunState::Running,
            task: None,
            started_unix_ms: 1,
            worked_ms: 0,
            working_since_unix_ms: Some(1),
            notify: cide_ipc::RunNotify::default(),
            stale_turn: false,
            note: None,
            openable: false,
        });
        h.host.agents.lock().push(cide_ipc::remote::RemoteAgent {
            id: "reviewer".to_owned().into(),
            label: "Reviewer".to_owned(),
            scope: cide_ipc::agents::AgentScope::Project,
            harness: cide_ipc::Harness::Claude,
            description: String::new(),
            model: None,
            color: None,
            unavailable: None,
            max_concurrent: 2,
            worktree: true,
            running: 1,
            queued: 0,
        });

        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        say(
            &mut ws,
            Some(2),
            ClientBody::Subscribe {
                projects: vec![first],
            },
        )
        .await;

        let mut saw_runs = false;
        let mut saw_roster = false;
        let mut saw_board = false;
        for _ in 0..12 {
            match heard_any(&mut ws).await {
                Some(ServerBody::Runs { project, runs }) => {
                    assert_eq!(project, first);
                    assert_eq!(runs.len(), 1);
                    // The label is the one copied at dispatch, which is what lets a finished
                    // run still read correctly after its role was renamed.
                    assert_eq!(runs[0].agent_label, "Reviewer");
                    saw_runs = true;
                }
                Some(ServerBody::Roster {
                    project,
                    agents,
                    dispatching,
                }) => {
                    // Carried for the reason its own doc gives: a paused project with nothing
                    // running is indistinguishable from an idle one without it. Asserted below,
                    // after the fake's queue is deliberately shut.
                    let _ = dispatching;
                    assert_eq!(project, first);
                    assert_eq!(agents.len(), 1);
                    assert_eq!(agents[0].running, 1);
                    saw_roster = true;
                }
                Some(ServerBody::Board { project, .. }) => {
                    assert_eq!(project, first);
                    saw_board = true;
                }
                Some(_) => {}
                None => break,
            }
            if saw_runs && saw_roster && saw_board {
                break;
            }
        }
        assert!(
            saw_runs && saw_roster && saw_board,
            "all three per-project reads are served"
        );

        // A paused project must *say* it is paused, and this is the assertion the first cut of
        // this feature could not make. Pausing does two things — shuts the dispatch queue and
        // freezes the children — and carrying only the second meant a project paused while
        // nothing happened to be running arrived on a device looking exactly like an idle one.
        *h.host.dispatching.lock() = false;
        say(
            &mut ws,
            Some(9),
            ClientBody::Subscribe {
                projects: vec![first],
            },
        )
        .await;
        let mut said_so = false;
        // Looped until the *new* answer arrives rather than trusting the first Roster read: the
        // previous subscribe's frames may still be in flight, and a test that asserts on whichever
        // arrives first is a test that fails on timing. Bounded by a **clock** and not by a frame
        // count, which is what it was: twelve frames is a guess about how much traffic a loaded
        // machine puts in front of the answer, and it was wrong on two runs in five of a parallel
        // suite while passing twelve times out of twelve alone. `heard_any` has its own
        // five-second patience per frame, so this cannot spin.
        let mut saw: Vec<String> = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            match heard_any(&mut ws).await {
                Some(ServerBody::Roster { dispatching, .. }) if !dispatching => {
                    said_so = true;
                    break;
                }
                // Kept, and named in the failure: a test that says only *it never came* leaves
                // the next reader unable to tell a budget that ran out from a socket that shut.
                Some(other) => {
                    let mut rendered = format!("{other:?}");
                    rendered.truncate(60);
                    saw.push(rendered);
                }
                None => {
                    saw.push("the socket closed".to_owned());
                    break;
                }
            }
        }
        assert!(
            said_so,
            "the roster says whether the queue is open; saw {saw:#?}"
        );
        *h.host.dispatching.lock() = true;

        // And a device that subscribes elsewhere is not served this project's, which is
        // `notify`'s addressed-never-broadcast rule applied to a read.
        h.host.asked.lock().clear();
        say(
            &mut ws,
            Some(3),
            ClientBody::Subscribe {
                projects: vec![second],
            },
        )
        .await;
        let mut narrowed = false;
        for _ in 0..12 {
            match heard_any(&mut ws).await {
                // The *new* project's answer, not whichever frame was still in flight from the
                // previous subscription.
                Some(ServerBody::Runs { project, runs }) if project == second => {
                    assert!(
                        runs.is_empty(),
                        "the other project's runs are not served here"
                    );
                    narrowed = true;
                    break;
                }
                Some(_) => {}
                None => break,
            }
        }
        assert!(
            narrowed,
            "a re-subscribe is answered for the project it named"
        );
    }

    async fn connect(h: &Harness) -> Client {
        shake(h, Mode::Pair, "", &[]).await
    }

    /// Open a socket and shake hands, in whichever mode and with whichever key.
    async fn shake(h: &Harness, mode: Mode, device: &str, psk: &[u8]) -> Client {
        let (mut ws, _) = tokio_tungstenite::connect_async(&h.url)
            .await
            .expect("connects");
        let (handshake, ephemeral) = crate::seal::client_handshake(mode, device);
        // Sent before the greeting is read, which is the ordering every client should use and
        // therefore the one the tests exercise: the two messages cross, and a client that waited
        // for the greeting first would pay a round trip for nothing.
        ws.send(Message::binary(handshake.encode()))
            .await
            .expect("sends the handshake");

        // Reading it is not politeness — it is the assertion that it was sent, and that the key
        // in it is this instance's. A device pairing by a typed address has nothing else to
        // derive against, so a server that stopped greeting would leave that road unable to
        // start while every scanned pairing went on working.
        let hailed = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("the server greets within five seconds")
            .expect("a message")
            .expect("not an error");
        let Message::Binary(bytes) = hailed else {
            panic!("the greeting was not binary");
        };
        let greeting = crate::seal::Greeting::decode(&bytes).expect("a greeting");
        assert_eq!(greeting.seal_version, crate::seal::SEAL_VERSION);
        assert_eq!(greeting.server_public, h.statik.public_bytes());

        let derived =
            crate::seal::derive_client(&ephemeral, &greeting.server_public, &handshake, psk);
        let (opener, sealer) = (derived.opener, derived.sealer);
        Client { ws, opener, sealer }
    }

    fn client_info() -> cide_ipc::remote::ClientInfo {
        cide_ipc::remote::ClientInfo {
            name: "Pixel 9".to_owned(),
            platform: "android".to_owned(),
            app_version: "0.1.0".to_owned(),
        }
    }

    async fn say(client: &mut Client, id: Option<u32>, body: ClientBody) {
        let frame = ClientFrame { id, body };
        let json = serde_json::to_vec(&frame).expect("serialises");
        let boxed = client.sealer.seal(&json).expect("seals");
        client.ws.send(Message::binary(boxed)).await.expect("sends");
    }

    /// The next frame, or `None` when the server closed.
    ///
    /// A **text** frame here is the one thing the server ever says in the clear: a refusal to a
    /// device it does not know. Parsed rather than ignored, so the test for it can read it.
    /// The next frame a test is *about*.
    ///
    /// The three per-project reads are skipped, because they are unsolicited pushes — like a
    /// `SessionState` that happens to land mid-test — and almost every test here reads frames
    /// positionally after a snapshot. A test that is about them uses [`heard_any`].
    ///
    /// Skipping rather than reordering the server: they genuinely do arrive at any time, and a
    /// test that only passes while they do not is a test that will fail the first time an agent
    /// changes state during it.
    async fn heard(client: &mut Client) -> Option<ServerBody> {
        loop {
            match heard_any(client).await? {
                ServerBody::Runs { .. } | ServerBody::Roster { .. } | ServerBody::Board { .. } => {
                    continue;
                }
                body => return Some(body),
            }
        }
    }

    /// Every frame, including the routine pushes.
    async fn heard_any(client: &mut Client) -> Option<ServerBody> {
        Some(heard_frame(client).await?.body)
    }

    /// The whole envelope, for the one claim that is about the `id` rather than the body.
    async fn heard_frame(client: &mut Client) -> Option<ServerFrame> {
        loop {
            let message = tokio::time::timeout(Duration::from_secs(5), client.ws.next())
                .await
                .expect("the server answered within five seconds")?;
            match message.ok()? {
                Message::Binary(bytes) => {
                    let plaintext = client.opener.open(&bytes).expect("opens");
                    let frame: ServerFrame =
                        serde_json::from_slice(&plaintext).expect("a known frame");
                    return Some(frame);
                }
                Message::Text(text) => {
                    let frame: ServerFrame = serde_json::from_str(&text).expect("a known frame");
                    return Some(frame);
                }
                Message::Close(_) => return None,
                _ => continue,
            }
        }
    }

    /// Pair a device and return its id and key.
    ///
    /// The key crosses the wire exactly once, here, **inside** a channel already established
    /// against the instance's public key — which is the one moment it is confidential and the
    /// only moment it is sent at all.
    async fn pair(h: &Harness) -> (String, Vec<u8>) {
        let code = h.devices.begin_pairing().expect("opens a pairing window");
        let mut client = shake(h, Mode::Pair, "", &[]).await;
        say(
            &mut client,
            Some(1),
            ClientBody::Pair {
                code: crate::devices::grouped(&code),
                client: client_info(),
            },
        )
        .await;
        match heard(&mut client).await.expect("an answer") {
            ServerBody::Paired { device, key, .. } => {
                let raw = (0..key.len() / 2)
                    .map(|i| u8::from_str_radix(&key[i * 2..i * 2 + 2], 16).expect("hex"))
                    .collect();
                (device, raw)
            }
            other => panic!("expected Paired, got {other:?}"),
        }
    }

    /// Open a resumed connection for a paired device.
    async fn resumed(h: &Harness, device: &str, key: &[u8]) -> Client {
        shake(h, Mode::Resume, device, key).await
    }

    async fn hello(client: &mut Client) -> ServerBody {
        say(
            client,
            Some(1),
            ClientBody::Hello {
                protocol: PROTOCOL_VERSION,
                client: client_info(),
            },
        )
        .await;
        heard(client).await.expect("an answer")
    }

    #[tokio::test]
    async fn a_paired_device_is_welcomed_and_handed_the_whole_board() {
        let (h, _first, _second) = harness().await;
        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;

        match hello(&mut ws).await {
            ServerBody::Welcome {
                protocol,
                instance,
                features,
            } => {
                assert_eq!(protocol, PROTOCOL_VERSION);
                // The *server* is authoritative for the name a device lists, never the pairing
                // payload — a profile renamed on the desktop must reach the phone on the next
                // connect.
                assert_eq!(instance.name, "[DEV] testbox");
                assert_eq!(instance.profile.as_deref(), Some("test"));
                assert!(features.contains(&"sessions".to_owned()));
            }
            other => panic!("expected Welcome, got {other:?}"),
        }

        // Projects, sessions and the awaiting set follow unasked, in that order: a device must
        // be able to paint a complete list from its first snapshot.
        match heard(&mut ws).await.expect("projects") {
            ServerBody::Projects { rev, projects } => {
                assert_eq!(rev, 7);
                assert_eq!(projects.len(), 2);
            }
            other => panic!("expected Projects, got {other:?}"),
        }
        match heard(&mut ws).await.expect("sessions") {
            ServerBody::Sessions { sessions } => assert_eq!(sessions.len(), 3),
            other => panic!("expected Sessions, got {other:?}"),
        }
        match heard(&mut ws).await.expect("awaiting") {
            ServerBody::Awaiting { entries } => assert_eq!(entries.len(), 1),
            other => panic!("expected Awaiting, got {other:?}"),
        }
    }

    /// The security property of the unauthenticated state, asserted on the host rather than on
    /// the error: nothing was *read*, not merely that something was refused.
    #[tokio::test]
    async fn an_unauthenticated_socket_may_pair_and_nothing_else() {
        let (h, first, _second) = harness().await;
        let mut ws = connect(&h).await;

        say(
            &mut ws,
            Some(1),
            ClientBody::Subscribe {
                projects: vec![first],
            },
        )
        .await;
        match heard(&mut ws).await.expect("a refusal") {
            ServerBody::Error { kind, .. } => assert_eq!(kind, error_kind::UNEXPECTED),
            other => panic!("expected an error, got {other:?}"),
        }
        assert!(
            heard(&mut ws).await.is_none(),
            "the socket stayed open after an unauthorised frame"
        );
        assert!(
            h.host.asked.lock().is_empty(),
            "an unauthenticated connection reached the host"
        );
    }

    /// A device this cide does not know is told so **in the clear**, which is the one thing the
    /// server ever says unsealed.
    ///
    /// Deliberate: a revoked phone would otherwise get a socket that opens and then goes silent,
    /// which is indistinguishable from a bad network. "You were removed" is a sentence somebody
    /// can act on, and it reveals nothing — the device id was in the handshake the client sent.
    #[tokio::test]
    async fn an_unknown_device_is_told_so_and_reads_nothing() {
        let (h, _first, _second) = harness().await;
        let mut ws = shake(&h, Mode::Resume, "d-nobody", &[7u8; 32]).await;

        match heard(&mut ws).await.expect("a refusal") {
            ServerBody::Error { kind, .. } => assert_eq!(kind, error_kind::UNAUTHORIZED),
            other => panic!("expected an error, got {other:?}"),
        }
        assert!(heard(&mut ws).await.is_none(), "the socket stayed open");
        assert!(h.host.asked.lock().is_empty());
    }

    /// The whole of authentication, over a real socket: a device with the wrong key is not
    /// *told* it is wrong — it is not understood, and the connection ends.
    #[tokio::test]
    async fn a_paired_device_with_the_wrong_key_is_not_understood() {
        let (h, _first, _second) = harness().await;
        let (device, _key) = pair(&h).await;

        let mut ws = shake(&h, Mode::Resume, &device, &[0u8; 32]).await;
        say(
            &mut ws,
            Some(1),
            ClientBody::Hello {
                protocol: PROTOCOL_VERSION,
                client: client_info(),
            },
        )
        .await;
        assert!(
            heard(&mut ws).await.is_none(),
            "a frame that could not be opened was answered"
        );
        assert!(h.host.asked.lock().is_empty());
    }

    /// A device built against a newer cide must be told which end is behind, in a sentence, and
    /// must not be left to guess from a dropped socket.
    #[tokio::test]
    async fn a_hello_from_the_future_is_refused_by_name() {
        let (h, _first, _second) = harness().await;
        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;

        say(
            &mut ws,
            Some(1),
            ClientBody::Hello {
                protocol: PROTOCOL_VERSION + 9,
                client: client_info(),
            },
        )
        .await;
        match heard(&mut ws).await.expect("a refusal") {
            ServerBody::Error { kind, detail } => {
                assert_eq!(kind, error_kind::PROTOCOL);
                assert!(detail.contains("update cide"), "{detail}");
            }
            other => panic!("expected an error, got {other:?}"),
        }
    }

    /// Not fatal, on purpose. A device built against a newer cide degrades rather than
    /// disconnecting — the same rule `cide-ide-mcp` states as "never break the terminal".
    #[tokio::test]
    async fn an_unreadable_frame_is_answered_and_the_connection_survives() {
        let (h, _first, _second) = harness().await;
        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        assert!(matches!(hello(&mut ws).await, ServerBody::Welcome { .. }));
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }

        // Sealed, and unreadable *inside* — which is what a device built against a newer cide
        // actually sends. A junk frame that did not seal would be refused by the transport, and
        // would prove nothing about the vocabulary degrading.
        let junk = ws
            .sealer
            .seal(br#"{"body":{"t":"somethingNewer"}}"#)
            .expect("seals");
        ws.ws.send(Message::binary(junk)).await.expect("sends");
        match heard(&mut ws).await.expect("an answer") {
            ServerBody::Error { kind, .. } => assert_eq!(kind, error_kind::UNKNOWN_FRAME),
            other => panic!("expected an error, got {other:?}"),
        }

        // And the connection is still usable, which is the whole claim.
        say(&mut ws, Some(2), ClientBody::Ping).await;
        assert!(matches!(
            heard(&mut ws).await.expect("a pong"),
            ServerBody::Pong
        ));
    }

    /// The refusal goes back to the question, not past it.
    ///
    /// A device keys its outstanding requests by the envelope's `id`; an answer with none is an
    /// event. So a refusal that dropped the id left the asker waiting out its own timeout and
    /// then saying cide had not answered — about a cide that answered at once and named the
    /// problem. Asserted on the id rather than on the kind, because the kind was always right.
    #[tokio::test]
    async fn an_unreadable_frame_is_refused_to_the_request_that_asked() {
        let (h, _first, _second) = harness().await;
        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        assert!(matches!(hello(&mut ws).await, ServerBody::Welcome { .. }));
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }

        let junk = ws
            .sealer
            .seal(br#"{"id":41,"body":{"t":"somethingNewer"}}"#)
            .expect("seals");
        ws.ws.send(Message::binary(junk)).await.expect("sends");
        // Past the routine pushes, which carry no id and are not what this is about.
        let frame = loop {
            let frame = heard_frame(&mut ws).await.expect("an answer");
            if !matches!(
                frame.body,
                ServerBody::Runs { .. } | ServerBody::Roster { .. } | ServerBody::Board { .. }
            ) {
                break frame;
            }
        };
        assert!(matches!(frame.body, ServerBody::Error { .. }));
        assert_eq!(frame.id, Some(41));
    }

    /// An id is recovered where there is one and never invented where there is not.
    ///
    /// The second half is the one that costs: a guessed id resolves whichever question happens
    /// to be outstanding, and the asker takes an answer to somebody else's frame as its own.
    #[test]
    fn an_envelope_id_survives_a_body_that_does_not() {
        assert_eq!(
            envelope_id(br#"{"id":7,"body":{"t":"somethingNewer"}}"#),
            Some(7)
        );
        assert_eq!(envelope_id(br#"{"body":{"t":"somethingNewer"}}"#), None);
        assert_eq!(envelope_id(br#"{"id":"seven","body":{}}"#), None);
        assert_eq!(envelope_id(br#"{"id":-1,"body":{}}"#), None);
        assert_eq!(envelope_id(b"not json at all"), None);
        assert_eq!(envelope_id(b"[]"), None);
    }

    /// `Subscribe` states what the set *is*. A second one narrows to the second project and does
    /// not accumulate the first — which is what makes re-sending the whole set after a reconnect
    /// a safe thing to do.
    #[tokio::test]
    async fn subscribe_replaces_the_set_rather_than_adding_to_it() {
        let (h, first, second) = harness().await;
        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }

        say(
            &mut ws,
            None,
            ClientBody::Subscribe {
                projects: vec![first],
            },
        )
        .await;
        heard(&mut ws).await.expect("projects");
        let narrowed = match heard(&mut ws).await.expect("sessions") {
            ServerBody::Sessions { sessions } => sessions,
            other => panic!("expected Sessions, got {other:?}"),
        };
        assert_eq!(narrowed.len(), 1);
        assert_eq!(narrowed[0].project, first);
        heard(&mut ws).await.expect("awaiting");

        say(
            &mut ws,
            None,
            ClientBody::Subscribe {
                projects: vec![second],
            },
        )
        .await;
        heard(&mut ws).await.expect("projects");
        let again = match heard(&mut ws).await.expect("sessions") {
            ServerBody::Sessions { sessions } => sessions,
            other => panic!("expected Sessions, got {other:?}"),
        };
        assert_eq!(again.len(), 2, "the first project's sessions came back too");
        assert!(again.iter().all(|s| s.project == second));

        assert_eq!(
            *h.host.asked.lock(),
            vec![None, Some(first), Some(second)],
            "the host was asked something other than what was subscribed"
        );
    }

    #[tokio::test]
    async fn a_wrong_code_burns_the_code() {
        let (h, _first, _second) = harness().await;
        let real = h.devices.begin_pairing().expect("opens");

        let mut ws = connect(&h).await;
        say(
            &mut ws,
            Some(1),
            ClientBody::Pair {
                code: "AAAA-AAAA".to_owned(),
                client: client_info(),
            },
        )
        .await;
        match heard(&mut ws).await.expect("a refusal") {
            ServerBody::Error { kind, .. } => assert_eq!(kind, error_kind::PAIRING),
            other => panic!("expected an error, got {other:?}"),
        }

        // The real code is now worthless. A retry budget is exactly what makes forty bits
        // guessable, so a wrong guess spends the code rather than the attacker's patience.
        let mut ws = connect(&h).await;
        say(
            &mut ws,
            Some(1),
            ClientBody::Pair {
                code: real,
                client: client_info(),
            },
        )
        .await;
        assert!(matches!(
            heard(&mut ws).await.expect("a refusal"),
            ServerBody::Error { .. }
        ));
        assert!(h.devices.devices().is_empty());
    }

    #[tokio::test]
    async fn an_expired_code_is_refused() {
        let (h, _first, _second) = harness().await;
        let code = h.devices.begin_pairing().expect("opens");
        h.devices.expire_pending_for_test();
        assert_eq!(
            h.devices.pending_code(),
            None,
            "an expired code is not open"
        );

        let mut ws = connect(&h).await;
        say(
            &mut ws,
            Some(1),
            ClientBody::Pair {
                code,
                client: client_info(),
            },
        )
        .await;
        match heard(&mut ws).await.expect("a refusal") {
            ServerBody::Error { kind, detail } => {
                assert_eq!(kind, error_kind::PAIRING);
                assert!(detail.contains("expired"), "{detail}");
            }
            other => panic!("expected an error, got {other:?}"),
        }
    }

    /// A revoked device is refused with the credential it still holds.
    #[tokio::test]
    async fn revoking_a_device_takes_its_key_with_it() {
        let (h, _first, _second) = harness().await;
        let (device, key) = pair(&h).await;
        assert!(h.devices.forget(&device).expect("writes"));

        // Revoking is removing, so there is no key to derive with and the refusal lands at the
        // handshake — which is why it is in the clear, and why it is a sentence.
        let mut ws = shake(&h, Mode::Resume, &device, &key).await;
        match heard(&mut ws).await.expect("a refusal") {
            ServerBody::Error { kind, .. } => assert_eq!(kind, error_kind::UNAUTHORIZED),
            other => panic!("expected an error, got {other:?}"),
        }
    }

    /// A device that is connected hears about a transition without asking.
    #[tokio::test]
    async fn a_session_transition_reaches_a_connected_device() {
        let (h, _first, _second) = harness().await;
        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }

        let session = SessionId::new();
        h.server.notify(RemoteEvent::SessionState {
            session,
            state: SessionState::AwaitingInput,
        });
        match heard(&mut ws).await.expect("a transition") {
            ServerBody::SessionState {
                session: got,
                state,
            } => {
                assert_eq!(got, session);
                assert_eq!(state, SessionState::AwaitingInput);
            }
            other => panic!("expected SessionState, got {other:?}"),
        }
    }

    /// The fan-out reaches *paired* devices. A socket that has connected and not proved anything
    /// is in the connection table so that closing it is one code path, and must be served nothing
    /// at all.
    #[tokio::test]
    async fn the_fan_out_skips_a_socket_that_has_not_said_hello() {
        let (h, _first, _second) = harness().await;
        let mut lurker = connect(&h).await;

        h.server.notify(RemoteEvent::Awaiting { entries: vec![] });
        h.server.notify(RemoteEvent::SessionState {
            session: SessionId::new(),
            state: SessionState::Busy,
        });

        // Nothing arrives; the only thing that ever will is the handshake timeout closing it.
        let quiet = tokio::time::timeout(Duration::from_millis(200), lurker.ws.next()).await;
        assert!(quiet.is_err(), "an unauthenticated socket was served news");
    }

    /// A run of workspace mutations — a pane drag is one — costs one re-read and one push, not
    /// one per mutation. And what is pushed is a *projection*: the tree itself never crosses.
    #[tokio::test]
    async fn a_burst_of_workspace_changes_costs_one_fresh_projection() {
        let (h, _first, _second) = harness().await;
        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }
        let after_snapshot = h.host.projects_asked.load(Ordering::Relaxed);

        for rev in 1..=8 {
            h.server.notify(RemoteEvent::WorkspaceRev { rev });
        }
        match heard(&mut ws).await.expect("projects") {
            ServerBody::Projects { rev, .. } => assert_eq!(rev, 7),
            other => panic!("expected Projects, got {other:?}"),
        }
        assert!(matches!(
            heard(&mut ws).await.expect("sessions"),
            ServerBody::Sessions { .. }
        ));
        assert_eq!(
            h.host.projects_asked.load(Ordering::Relaxed) - after_snapshot,
            1,
            "eight mutations were re-read eight times"
        );

        // And nothing further follows, because nothing further changed.
        let quiet = tokio::time::timeout(Duration::from_millis(600), ws.ws.next()).await;
        assert!(quiet.is_err(), "the coalescer pushed with nothing dirty");
    }

    /// The bounded queue's whole purpose: a device that stops reading is told it missed
    /// something, and the thing that noticed the event is never made to wait for it.
    #[tokio::test]
    async fn a_device_that_stops_reading_is_told_it_fell_behind() {
        let (h, _first, _second) = harness().await;
        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }

        // Far more than the socket buffer and the queue between them can hold, pushed from this
        // task without ever awaiting the device — which is exactly the shape of the real caller.
        let session = SessionId::new();
        for _ in 0..20_000 {
            h.server.notify(RemoteEvent::SessionState {
                session,
                state: SessionState::Busy,
            });
        }

        // Now read. A `Desync` must appear, and the connection must still work afterwards.
        let mut saw_desync = false;
        for _ in 0..20_000 {
            match heard(&mut ws).await {
                Some(ServerBody::Desync { .. }) => {
                    saw_desync = true;
                    break;
                }
                Some(_) => continue,
                None => break,
            }
        }
        assert!(saw_desync, "a device that stopped reading was never told");
    }

    fn grid(rows: &[&str]) -> cide_ipc::screen::ScreenCapture {
        use cide_ipc::screen::{ScreenInfo, ScreenLine, StyleRun};
        cide_ipc::screen::ScreenCapture {
            info: ScreenInfo {
                cols: 20,
                rows: rows.len() as u16,
                alt: false,
                app_cursor: false,
                bracketed_paste: false,
                mouse: cide_ipc::screen::MouseReporting::Off,
            },
            cursor: None,
            lines: rows
                .iter()
                .enumerate()
                .map(|(i, text)| ScreenLine {
                    row: i as u16,
                    wrapped: false,
                    runs: vec![StyleRun {
                        text: (*text).to_owned(),
                        fg: None,
                        bg: None,
                        flags: 0,
                    }],
                })
                .collect(),
        }
    }

    /// Nothing watched, nothing read. This is the state a device is in whenever it is looking at
    /// a list rather than a terminal, which is most of the time, and it must cost cide nothing.
    #[tokio::test]
    async fn a_screen_nobody_is_watching_is_never_read() {
        let (h, _first, _second) = harness().await;
        let session = SessionId::new();
        h.host.screens.lock().insert(session, grid(&["hello"]));

        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }

        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(h.host.screens_read.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn watching_sends_the_whole_grid_and_then_only_what_moved() {
        let (h, _first, _second) = harness().await;
        let session = SessionId::new();
        h.host
            .screens
            .lock()
            .insert(session, grid(&["a", "b", "c"]));

        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }

        say(&mut ws, None, ClientBody::WatchScreen { session }).await;
        let first = match heard(&mut ws).await.expect("a screen") {
            ServerBody::Screen { update } => update,
            other => panic!("expected Screen, got {other:?}"),
        };
        assert!(first.full);
        assert_eq!(first.session, session);
        assert_eq!(first.lines.len(), 3);
        assert_eq!(first.info.cols, 20);

        // One line changes. The other two must not come back.
        h.host
            .screens
            .lock()
            .insert(session, grid(&["a", "B", "c"]));
        let next = match heard(&mut ws).await.expect("a repaint") {
            ServerBody::Screen { update } => update,
            other => panic!("expected Screen, got {other:?}"),
        };
        assert!(!next.full);
        assert_eq!(next.epoch, first.epoch, "the grid changed identity");
        assert_eq!(next.lines.len(), 1);
        assert_eq!(next.lines[0].row, 1);

        // And unwatching stops the reads.
        say(&mut ws, None, ClientBody::UnwatchScreen { session }).await;
        tokio::time::sleep(Duration::from_millis(250)).await;
        let settled = h.host.screens_read.load(Ordering::Relaxed);
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(h.host.screens_read.load(Ordering::Relaxed), settled);
    }

    /// A terminal that stopped changing and a terminal whose child died are the same picture on
    /// a phone, so the difference has to be said rather than left to silence.
    #[tokio::test]
    async fn a_session_that_goes_away_is_said_out_loud() {
        let (h, _first, _second) = harness().await;
        let session = SessionId::new();
        h.host.screens.lock().insert(session, grid(&["a"]));

        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }
        say(&mut ws, None, ClientBody::WatchScreen { session }).await;
        assert!(matches!(
            heard(&mut ws).await.expect("a screen"),
            ServerBody::Screen { .. }
        ));

        h.host.screens.lock().remove(&session);
        match heard(&mut ws).await.expect("an ending") {
            ServerBody::ScreenGone { session: gone } => assert_eq!(gone, session),
            other => panic!("expected ScreenGone, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_scrollback_page_is_answered_and_an_unknown_session_is_refused() {
        let (h, _first, _second) = harness().await;
        let session = SessionId::new();
        h.host
            .scrollback
            .lock()
            .insert(session, grid(&["one", "two", "three", "four"]).lines);

        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }

        say(
            &mut ws,
            Some(9),
            ClientBody::ScrollbackPage {
                session,
                from_top: 1,
                rows: 2,
            },
        )
        .await;
        match heard(&mut ws).await.expect("a page") {
            ServerBody::Scrollback { session: got, page } => {
                assert_eq!(got, session);
                assert_eq!(page.depth, 4);
                assert_eq!(page.from_top, 1);
                assert_eq!(page.lines.len(), 2);
                assert_eq!(page.lines[0].runs[0].text, "two");
            }
            other => panic!("expected Scrollback, got {other:?}"),
        }

        say(
            &mut ws,
            Some(10),
            ClientBody::ScrollbackPage {
                session: SessionId::new(),
                from_top: 0,
                rows: 2,
            },
        )
        .await;
        match heard(&mut ws).await.expect("a refusal") {
            ServerBody::Error { kind, .. } => assert_eq!(kind, error_kind::NO_SUCH),
            other => panic!("expected an error, got {other:?}"),
        }
    }

    /// A scroll reaches the child as a wheel — and only where the child asked for one.
    ///
    /// Both halves in one test, because they are one decision. The refusal is the half worth
    /// having: a program that never enabled mouse reports reads these bytes as typed characters,
    /// so a phone scrolling a shell would be typing `[<64;…M` into it. Asserted on **what was
    /// written**, since a refusal that still wrote something would satisfy any check on the reply.
    #[tokio::test]
    async fn a_scroll_is_a_wheel_where_the_child_asked_and_nothing_where_it_did_not() {
        let (h, _first, _second) = harness().await;
        let session = SessionId::new();
        h.host.screens.lock().insert(session, grid(&["prompt"]));

        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }

        // Mouse reporting off, which is `grid`'s default and every shell's state.
        say(
            &mut ws,
            Some(7),
            ClientBody::Scroll {
                session,
                lines: -3,
                seq: 1,
            },
        )
        .await;
        match heard(&mut ws).await.expect("an answer") {
            ServerBody::Error { kind, .. } => assert_eq!(kind, error_kind::REFUSED),
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert!(
            h.host.written.lock().is_empty(),
            "a refused scroll still reached the child"
        );

        // And with tracking on, as `claude` turns it on.
        {
            let mut screens = h.host.screens.lock();
            let screen = screens.get_mut(&session).expect("the screen");
            screen.info.mouse = cide_ipc::screen::MouseReporting::Sgr;
        }
        say(
            &mut ws,
            None,
            ClientBody::Scroll {
                session,
                lines: -2,
                seq: 2,
            },
        )
        .await;
        say(&mut ws, Some(8), ClientBody::Ping).await;
        loop {
            match heard(&mut ws).await.expect("an answer") {
                ServerBody::Pong => break,
                _ => continue,
            }
        }
        let written = h.host.written.lock();
        assert_eq!(written.len(), 1, "one write for one scroll");
        assert_eq!(
            String::from_utf8_lossy(&written[0].bytes),
            "\x1b[<64;11;1M\x1b[<64;11;1M"
        );
    }

    /// A device sends *up*; what reaches the child depends on a mode only the mirror knows.
    #[tokio::test]
    async fn a_keystroke_is_encoded_against_the_childs_live_modes() {
        use cide_ipc::remote::{KeyEvent, KeyName};

        let (h, _first, _second) = harness().await;
        let session = SessionId::new();
        h.host.screens.lock().insert(session, grid(&["prompt"]));

        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }

        let up = KeyEvent {
            key: KeyName::Up,
            text: None,
            ctrl: false,
            alt: false,
            shift: false,
        };
        say(
            &mut ws,
            None,
            ClientBody::Input {
                session,
                key: up.clone(),
                seq: 1,
                expect_screen: None,
            },
        )
        .await;
        // Round-trip a ping so the input is certainly handled before the assertion.
        say(&mut ws, Some(1), ClientBody::Ping).await;
        loop {
            match heard(&mut ws).await.expect("an answer") {
                ServerBody::Pong => break,
                _ => continue,
            }
        }
        assert_eq!(h.host.written.lock()[0].bytes, b"\x1b[A".to_vec());

        // The child turns DECCKM on. The *same* key must now be encoded the other way, and the
        // device was never told.
        let mut app = grid(&["prompt"]);
        app.info.app_cursor = true;
        h.host.screens.lock().insert(session, app);

        say(
            &mut ws,
            None,
            ClientBody::Input {
                session,
                key: up,
                seq: 2,
                expect_screen: None,
            },
        )
        .await;
        say(&mut ws, Some(2), ClientBody::Ping).await;
        loop {
            match heard(&mut ws).await.expect("an answer") {
                ServerBody::Pong => break,
                _ => continue,
            }
        }
        let written = h.host.written.lock();
        assert_eq!(
            written[1].bytes,
            b"\x1bOA".to_vec(),
            "the mode was read once and cached"
        );
        assert!(
            written[0].writer.starts_with("remote:"),
            "{}",
            written[0].writer
        );
        assert_eq!(
            written[1].seq, 2,
            "the sequence number did not reach the watermark"
        );
    }

    #[tokio::test]
    async fn a_paste_is_bracketed_from_the_childs_mode_and_not_the_devices_opinion() {
        let (h, _first, _second) = harness().await;
        let session = SessionId::new();
        let mut screen = grid(&["prompt"]);
        screen.info.bracketed_paste = true;
        h.host.screens.lock().insert(session, screen);

        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }

        say(
            &mut ws,
            None,
            ClientBody::Paste {
                session,
                text: "hello".to_owned(),
                seq: 1,
                expect_screen: None,
            },
        )
        .await;
        say(&mut ws, Some(1), ClientBody::Ping).await;
        loop {
            match heard(&mut ws).await.expect("an answer") {
                ServerBody::Pong => break,
                _ => continue,
            }
        }
        assert_eq!(
            String::from_utf8_lossy(&h.host.written.lock()[0].bytes),
            "\x1b[200~hello\x1b[201~"
        );
    }

    /// Typing into a session that is no longer there is a sentence, not silence. On a phone,
    /// silence looks exactly like a key that did not register.
    #[tokio::test]
    async fn typing_into_a_vanished_session_is_refused_by_name() {
        use cide_ipc::remote::{KeyEvent, KeyName};

        let (h, _first, _second) = harness().await;
        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }

        say(
            &mut ws,
            Some(3),
            ClientBody::Input {
                session: SessionId::new(),
                key: KeyEvent {
                    key: KeyName::Enter,
                    text: None,
                    ctrl: false,
                    alt: false,
                    shift: false,
                },
                seq: 1,
                expect_screen: None,
            },
        )
        .await;
        match heard(&mut ws).await.expect("a refusal") {
            ServerBody::Error { kind, .. } => assert_eq!(kind, error_kind::NO_SUCH),
            other => panic!("expected an error, got {other:?}"),
        }
        assert!(h.host.written.lock().is_empty());
    }

    /// The phone joins the finished-mark as a third surface, and reports **one session** — never
    /// its own idea of the whole set.
    #[tokio::test]
    async fn opening_a_session_on_a_device_acknowledges_exactly_that_session() {
        let (h, _first, _second) = harness().await;
        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }

        let session = SessionId::new();
        say(&mut ws, None, ClientBody::Acknowledge { session }).await;
        say(&mut ws, Some(1), ClientBody::Ping).await;
        loop {
            match heard(&mut ws).await.expect("an answer") {
                ServerBody::Pong => break,
                _ => continue,
            }
        }
        assert_eq!(*h.host.acknowledged.lock(), vec![session]);
    }

    #[tokio::test]
    async fn run_control_reaches_the_host_and_a_refusal_comes_back_as_a_sentence() {
        let (h, first, _second) = harness().await;
        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }

        let run = cide_ipc::RunId::new();
        say(
            &mut ws,
            None,
            ClientBody::RunStop {
                project: first,
                run,
                reason: Some("done for today".to_owned()),
                force: false,
            },
        )
        .await;
        say(
            &mut ws,
            None,
            ClientBody::RunPause {
                project: first,
                run: None,
            },
        )
        .await;
        say(
            &mut ws,
            None,
            ClientBody::RunResume {
                project: first,
                run: Some(run),
            },
        )
        .await;
        // A ping behind them: it can only be answered after the three were handled, and a
        // successful gesture answers with nothing at all, so anything but a pong is a bug.
        say(&mut ws, Some(1), ClientBody::Ping).await;
        match heard(&mut ws).await.expect("an answer") {
            ServerBody::Pong => {}
            other => panic!("a successful gesture answered with {other:?}"),
        }

        let calls = h.run_calls();
        assert_eq!(calls.len(), 3);
        assert!(
            calls[0].starts_with(&format!("stop {run} force=false")),
            "{}",
            calls[0]
        );
        assert!(calls[0].contains("done for today"));
        assert_eq!(calls[1], "pause None", "the project scope was lost");
        assert_eq!(calls[2], format!("resume Some({run:?})"));

        // A refusal is the host's own sentence, carried in `detail` where a person reads it.
        say(
            &mut ws,
            Some(2),
            ClientBody::RunStop {
                project: ProjectId::new(),
                run,
                reason: None,
                force: true,
            },
        )
        .await;
        match heard(&mut ws).await.expect("a refusal") {
            ServerBody::Error { kind, detail } => {
                assert_eq!(kind, error_kind::REFUSED);
                assert_eq!(detail, "no such project");
            }
            other => panic!("expected an error, got {other:?}"),
        }
    }

    /// The three orchestration gestures a device may make, and the shape of their answers:
    /// a dispatch answers with the run so the device can open what it just started, and the two
    /// task writes answer with nothing, because the new board arrives on its own.
    #[tokio::test]
    async fn dispatch_answers_with_a_run_and_a_task_write_answers_with_nothing() {
        let (h, first, _second) = harness().await;
        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }

        say(
            &mut ws,
            Some(1),
            ClientBody::Dispatch {
                request: cide_ipc::DispatchRequest {
                    project: first,
                    agent: cide_ipc::AgentId("reviewer".to_owned()),
                    task: Some(cide_ipc::TaskId("t-1".to_owned())),
                    prompt: None,
                    notify: None,
                },
            },
        )
        .await;
        match heard(&mut ws).await.expect("an answer") {
            ServerBody::Dispatched { .. } => {}
            other => panic!("expected Dispatched, got {other:?}"),
        }

        say(
            &mut ws,
            None,
            ClientBody::TaskNew {
                task: cide_ipc::TaskNew {
                    project: first,
                    title: "look at the thing".to_owned(),
                    body: None,
                    agent: None,
                    status: None,
                    attachments: None,
                    links: None,
                    change: None,
                },
            },
        )
        .await;
        say(&mut ws, Some(2), ClientBody::Ping).await;
        match heard(&mut ws).await.expect("an answer") {
            ServerBody::Pong => {}
            other => panic!("a task write answered with {other:?}"),
        }

        let calls = h.host.task_calls.lock().clone();
        assert_eq!(calls, vec!["dispatch reviewer", "new look at the thing"]);
    }

    fn a_prompt(digest: &str) -> cide_ipc::remote::PermissionPrompt {
        cide_ipc::remote::PermissionPrompt {
            question: vec!["Do you want to proceed?".to_owned()],
            options: vec![
                cide_ipc::remote::PromptOption {
                    number: 1,
                    label: "Yes".to_owned(),
                },
                cide_ipc::remote::PromptOption {
                    number: 2,
                    label: "No, and tell Claude what to do differently".to_owned(),
                },
            ],
            selected: Some(1),
            digest: digest.to_owned(),
        }
    }

    /// The single most important rule in the feature, from the server's side.
    ///
    /// Over a network the prompt a thumb is travelling towards can already have been replaced by
    /// the next one. Without the digest, a tap on "No, tell Claude what to do differently" lands
    /// as "1. Yes" on a question the user never saw — and the assertion that says so is that the
    /// host recorded **no answer**, not merely that an error came back.
    #[tokio::test]
    async fn an_answer_to_a_prompt_that_moved_is_refused_and_nothing_is_answered() {
        let (h, _first, _second) = harness().await;
        let session = SessionId::new();
        h.host.prompts.lock().insert(session, a_prompt("abc123"));

        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }

        // The prompt the device was shown is gone; a different question is on screen now.
        h.host.prompts.lock().insert(session, a_prompt("def456"));
        say(
            &mut ws,
            Some(1),
            ClientBody::AnswerPrompt {
                session,
                option: 1,
                expect_screen: "abc123".to_owned(),
            },
        )
        .await;
        match heard(&mut ws).await.expect("a refusal") {
            ServerBody::Error { kind, detail } => {
                assert_eq!(kind, error_kind::REFUSED);
                assert!(detail.contains("changed"), "{detail}");
            }
            other => panic!("expected an error, got {other:?}"),
        }
        assert!(
            h.host.answered.lock().is_empty(),
            "an answer reached the session after the prompt had moved"
        );

        // And the current one is answerable.
        say(
            &mut ws,
            Some(2),
            ClientBody::AnswerPrompt {
                session,
                option: 2,
                expect_screen: "def456".to_owned(),
            },
        )
        .await;
        say(&mut ws, Some(3), ClientBody::Ping).await;
        match heard(&mut ws).await.expect("an answer") {
            ServerBody::Pong => {}
            other => panic!("a successful answer replied with {other:?}"),
        }
        assert_eq!(*h.host.answered.lock(), vec![(session, 2)]);
    }

    /// A stray Enter from a soft keyboard, landing on a prompt nobody read.
    ///
    /// Free text is legitimate at a permission prompt — that is what its third option is for — so
    /// this is not a refusal of typing. It is a refusal of typing that cannot say which question
    /// it is answering.
    #[tokio::test]
    async fn typing_at_a_prompt_must_name_the_prompt() {
        use cide_ipc::remote::{KeyEvent, KeyName};

        let (h, _first, _second) = harness().await;
        let session = SessionId::new();
        h.host.screens.lock().insert(session, grid(&["prompt"]));
        h.host.prompts.lock().insert(session, a_prompt("abc123"));

        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }

        let enter = KeyEvent {
            key: KeyName::Enter,
            text: None,
            ctrl: false,
            alt: false,
            shift: false,
        };
        say(
            &mut ws,
            Some(1),
            ClientBody::Input {
                session,
                key: enter.clone(),
                seq: 1,
                expect_screen: None,
            },
        )
        .await;
        match heard(&mut ws).await.expect("a refusal") {
            ServerBody::Error { kind, .. } => assert_eq!(kind, error_kind::REFUSED),
            other => panic!("expected an error, got {other:?}"),
        }
        assert!(
            h.host.written.lock().is_empty(),
            "a bare Enter reached a prompt"
        );

        // Naming the wrong prompt is refused too.
        say(
            &mut ws,
            Some(2),
            ClientBody::Input {
                session,
                key: enter.clone(),
                seq: 2,
                expect_screen: Some("stale".to_owned()),
            },
        )
        .await;
        assert!(matches!(
            heard(&mut ws).await.expect("a refusal"),
            ServerBody::Error { .. }
        ));
        assert!(h.host.written.lock().is_empty());

        // Naming the right one goes through, because redirection is a real thing to want.
        say(
            &mut ws,
            Some(3),
            ClientBody::Input {
                session,
                key: enter,
                seq: 3,
                expect_screen: Some("abc123".to_owned()),
            },
        )
        .await;
        say(&mut ws, Some(4), ClientBody::Ping).await;
        match heard(&mut ws).await.expect("an answer") {
            ServerBody::Pong => {}
            other => panic!("an accepted keystroke replied with {other:?}"),
        }
        assert_eq!(h.host.written.lock().len(), 1);
    }

    /// A session that is not asking anything costs no digest and no host call for one.
    #[tokio::test]
    async fn typing_into_an_ordinary_session_needs_no_digest() {
        use cide_ipc::remote::{KeyEvent, KeyName};

        let (h, _first, _second) = harness().await;
        let session = SessionId::new();
        h.host.screens.lock().insert(session, grid(&["prompt"]));

        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }

        say(
            &mut ws,
            None,
            ClientBody::Input {
                session,
                key: KeyEvent {
                    key: KeyName::Char,
                    text: Some("x".to_owned()),
                    ctrl: false,
                    alt: false,
                    shift: false,
                },
                seq: 1,
                expect_screen: None,
            },
        )
        .await;
        say(&mut ws, Some(1), ClientBody::Ping).await;
        match heard(&mut ws).await.expect("an answer") {
            ServerBody::Pong => {}
            other => panic!("an ordinary keystroke replied with {other:?}"),
        }
        assert_eq!(h.host.written.lock()[0].bytes, b"x".to_vec());
    }

    /// A watching device is told what the session is asking, and told when it stops asking.
    #[tokio::test]
    async fn a_watcher_is_sent_the_prompt_and_then_told_it_is_gone() {
        let (h, _first, _second) = harness().await;
        let session = SessionId::new();
        h.host.screens.lock().insert(session, grid(&["asking"]));
        h.host.prompts.lock().insert(session, a_prompt("abc123"));

        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;
        for _ in 0..3 {
            heard(&mut ws).await.expect("the snapshot");
        }
        say(&mut ws, None, ClientBody::WatchScreen { session }).await;

        // The rows first, then the card — so a device drawing one over the other has the
        // terminal underneath it.
        assert!(matches!(
            heard(&mut ws).await.expect("a screen"),
            ServerBody::Screen { .. }
        ));
        match heard(&mut ws).await.expect("a prompt") {
            ServerBody::Prompt {
                session: got,
                prompt,
            } => {
                assert_eq!(got, session);
                assert_eq!(prompt.options.len(), 2);
                assert_eq!(prompt.digest, "abc123");
            }
            other => panic!("expected Prompt, got {other:?}"),
        }

        // It is not re-sent while it is the same question.
        let quiet = tokio::time::timeout(Duration::from_millis(400), ws.ws.next()).await;
        assert!(quiet.is_err(), "the same prompt was sent twice");

        h.host.prompts.lock().remove(&session);
        match heard(&mut ws).await.expect("an ending") {
            ServerBody::PromptGone { session: gone } => assert_eq!(gone, session),
            other => panic!("expected PromptGone, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn shutting_down_lets_a_live_connection_finish() {
        let (h, _first, _second) = harness().await;
        let (device, key) = pair(&h).await;
        let mut ws = resumed(&h, &device, &key).await;
        hello(&mut ws).await;

        let Harness { server, .. } = h;
        // Close from the client side so the connection task ends and `drained` can complete;
        // what is asserted is that `shutdown` waits for it rather than returning into a runtime
        // that is about to be dropped underneath a live task.
        ws.ws.close(None).await.expect("closes");
        tokio::time::timeout(Duration::from_secs(3), server.shutdown())
            .await
            .expect("shutdown returned within the drain timeout");
    }
}
