//! Spawning a language server and keeping it alive, or deciding not to.
//!
//! Three threads per server, and the division is the same one `cide-pty` makes:
//!
//! ```text
//! supervisor ── spawn ─┬─ reader thread  → codec::read_message → Session → events out
//!                      ├─ writer: drains the outbound queue → codec::write_message
//!                      └─ stderr drain  → tracing, keeping the last KEEP_STDERR bytes
//! ```
//!
//! The supervisor thread is the one that forks, and it blocks until the child is gone — which is
//! what makes [`cide_core::child_env::arm`] safe here. See the crate docs.

use std::collections::HashMap;
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cide_ipc::SourceStatus;
use crossbeam_channel::{Receiver, Sender, TrySendError};
use serde_json::Value;

use crate::codec;
use crate::discover::{Found, Server};
use crate::session::{Effect, Session};

/// How long to wait for `shutdown`'s reply, then for the process to exit on its own.
const GRACE: Duration = Duration::from_secs(2);
/// And after `SIGTERM`, before `SIGKILL`.
const KILL_AFTER: Duration = Duration::from_secs(1);

/// How much of a crashed server's stderr to keep for the report.
///
/// Enough to name the cause, not enough to paste the user's environment into a toast — the same
/// judgement `cide_claude::headless` makes about its own children.
const KEEP_STDERR: usize = 400;

/// Restart backoff, and then the point at which restarting stops.
const BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];
/// Three crashes inside this window stops the restarts for the life of the project.
///
/// An OOM-killed rust-analyzer on a huge repository would otherwise be respawned for ever,
/// re-indexing each time, and the machine would never recover.
const CRASH_WINDOW: Duration = Duration::from_secs(300);

/// How long a `Ready` must hold before it is believed.
///
/// **Measured, not guessed.** rust-analyzer's indexing is not one progress token — it is a
/// sequence of them (`Fetching`, `cargo metadata`, `Building CrateGraph`, `Roots Scanned`, …),
/// and in the gaps *between* them nothing is in flight. A rule of "handshake done and no token
/// outstanding" therefore reports `Ready` in every gap: against this workspace it flapped eight
/// times in the first 0.9 seconds before settling at 14.9s.
///
/// That is not a cosmetic flicker. `Ready` with an empty list is a **clean bill of health**, so
/// the panel would have announced "No problems found" eight times while the analyser was still
/// reading the crate graph — the exact confident-empty-list failure `cide_core::diagnostics`
/// exists to prevent, arriving through the one path that bypasses it.
///
/// One second, because every observed gap was ≤ 250 ms and the true settle was 14 seconds later:
/// long enough to bridge the whole noisy phase, short enough that a genuinely finished server is
/// reported promptly. `Scanning` is **not** delayed — only the claim that there is nothing left
/// to wait for.
const READY_SETTLE: Duration = Duration::from_secs(1);

/// How finely the restart backoff is sliced.
///
/// The backoff has to be *waited out* (see the loop that uses this) while still noticing a
/// shutdown promptly. Short enough that a quit is not visibly delayed, long enough not to spin.
const BACKOFF_TICK: Duration = Duration::from_millis(100);

/// Bound on the outbound queue.
///
/// A `didChange` per debounce interval and a handful of lifecycle messages is a trickle; a queue
/// this deep means the server has stopped reading, which is a wedged server rather than
/// backpressure worth propagating. Full means drop, with a line, rather than block the UI thread
/// that is sending.
const OUTBOX: usize = 256;

#[derive(Debug, thiserror::Error)]
pub enum LspError {
    #[error("{0}")]
    Unavailable(String),
    #[error("could not start {binary}: {message}")]
    Spawn { binary: String, message: String },
}

/// What a running server tells the app.
#[derive(Debug, Clone)]
pub enum LspEvent {
    /// This source's status changed.
    Status(SourceStatus),
    /// Replace one file's diagnostics for this source. Already converted; `rel` still has to be
    /// filled in by the app, which is the only layer that knows the project's roots.
    Published {
        abs_path: String,
        items: Vec<lsp_types::Diagnostic>,
    },
}

/// Why a request did not produce an answer.
///
/// Three variants and not one string, because the caller renders each differently and a user who
/// is told "no definition found" when the truth was "the server is still indexing" learns to
/// distrust the feature. See `ProjectDiagnostics::definition`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// The deadline passed with no reply. The server is alive but busy — almost always indexing.
    Timeout,
    /// The server stopped, or never started, while this request was outstanding.
    ServerGone,
    /// The outbound queue was full, so the request was never written. Distinguished from
    /// [`RequestError::Timeout`] because it is a *local* saturation and retrying is reasonable —
    /// see [`OUTBOX`], whose `send` drops a message in the same situation with only a log line.
    Queue,
    /// The server answered, and the answer was a JSON-RPC error. Carries its message, because that
    /// text is the server's own explanation and is worth more than any sentence we could invent.
    Failed(String),
}

impl std::fmt::Display for RequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "the language server did not answer in time"),
            Self::ServerGone => write!(f, "the language server stopped"),
            Self::Queue => write!(f, "the outbound queue to the language server was full"),
            Self::Failed(message) => write!(f, "{message}"),
        }
    }
}

/// Waiters, by request id.
type Pending = Arc<parking_lot::Mutex<HashMap<i64, Sender<Result<Value, RequestError>>>>>;

/// The first id this crate allocates for a caller's request.
///
/// **Not 1, and not 2.** `Session::new` hardcodes `initialize_id: 1` and `next_id: 2`, and a fresh
/// `Session` is built for *every life* of *every* server — so those two ids are re-issued after
/// each restart. An allocator that could hand out either would eventually have a caller's request
/// resolved by the handshake's reply, which is the kind of bug that appears once a week under load
/// and never in a test. Starting well above them costs nothing and makes the overlap impossible.
const REQUEST_ID_BASE: i64 = 100;

/// Sends requests to a server and waits for the reply.
///
/// Separate from [`LspHandle`], and cloneable, for one specific reason: `cide-app` keeps a
/// project's handles behind a mutex that *that project's* diagnostics pump takes every 100 ms and
/// that `restart` takes from a command thread. Waiting for a reply while holding it would stall
/// that project's diagnostics for the length of the wait — other projects have their own
/// `ProjectDiagnostics` and are unaffected — and would block against a concurrent `restart` until
/// the wait finished, which with a five-second deadline is a five-second freeze rather than a
/// permanent deadlock. Neither is acceptable behind a keystroke. A `Requester` is cloned out from
/// under the lock, the guard is dropped, and only then does anyone wait.
#[derive(Clone)]
pub struct Requester {
    server: Server,
    outbox: Sender<Value>,
    next_id: Arc<std::sync::atomic::AtomicI64>,
    pending: Pending,
}

impl Requester {
    pub fn server(&self) -> Server {
        self.server
    }

    /// Send one request and wait up to `timeout` for its reply.
    ///
    /// Registers the waiter **before** the request is written, because the reader thread can
    /// deliver the reply before this function reaches its `recv` — the pending map, not the order
    /// of operations, is what makes that safe.
    ///
    /// Every exit path removes the entry. A leaked entry is a slow memory leak *and* a waiter that
    /// a later id collision could resolve.
    pub fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, RequestError> {
        let id = self
            .next_id
            .fetch_add(1, Ordering::Relaxed)
            .max(REQUEST_ID_BASE);
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.pending.lock().insert(id, tx);

        let message = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        // `try_send` and not `send`: a full outbox must be an error the caller can render, not a
        // block. `LspHandle::send` drops in this situation with a `warn!`, which is right for a
        // notification nobody is waiting on and wrong for a request somebody is.
        if let Err(error) = self.outbox.try_send(message) {
            self.pending.lock().remove(&id);
            return Err(match error {
                TrySendError::Full(_) => RequestError::Queue,
                TrySendError::Disconnected(_) => RequestError::ServerGone,
            });
        }

        let answer = match rx.recv_timeout(timeout) {
            Ok(answer) => answer,
            // The sender was dropped without answering — the pending map was drained because the
            // server's life ended. `RecvTimeoutError::Timeout` is the real deadline.
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => Err(RequestError::ServerGone),
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => Err(RequestError::Timeout),
        };
        self.pending.lock().remove(&id);
        answer
    }
}

/// Resolve a waiter if this message is a reply to one. `true` when it was consumed.
///
/// A message with an `id` and no `method` is a response. Anything else — a notification, or a
/// server→client *request*, which also carries an id but has a method — must fall through to
/// `Session`, or `workspace/configuration` would go unanswered and rust-analyzer would stall
/// for ever waiting on it.
fn take_reply(pending: &Pending, message: &Value) -> bool {
    if message.get("method").is_some() {
        return false;
    }
    let Some(id) = message.get("id").and_then(Value::as_i64) else {
        return false;
    };
    // Taken out of the map here rather than by the waiter, so that a reply arriving twice — which
    // a misbehaving server may do — cannot resolve a *later* request that reused the id.
    let Some(waiter) = pending.lock().remove(&id) else {
        return false;
    };
    let answer = match message.get("error") {
        Some(error) => Err(RequestError::Failed(
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("the language server reported an error")
                .to_string(),
        )),
        // A response with neither `result` nor `error` is malformed; `null` is the correct and
        // common answer to `textDocument/definition`, so absent is treated as `null` rather than
        // as a failure.
        None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
    };
    // `try_send` on a `bounded(1)` nobody is reading any more (the caller timed out and left):
    // dropping the answer is correct and must not block the pump.
    let _ = waiter.try_send(answer);
    true
}

/// Answer every outstanding request with `error`, and forget them.
///
/// Called on **every** path a server's life can end. Missing one leaves a caller blocked until its
/// own deadline with no way to know the server is gone — the discipline `cide-ide-mcp`'s
/// `DiffBroker` states for the same shape of problem, where the cost of missing a path is an agent
/// that waits for ever.
fn cancel_pending(pending: &Pending, error: RequestError) {
    for (_, waiter) in pending.lock().drain() {
        let _ = waiter.try_send(Err(error.clone()));
    }
}

/// A running (or permanently stopped) language server.
///
/// Dropping it runs the shutdown ladder.
pub struct LspHandle {
    server: Server,
    outbox: Sender<Value>,
    events: Receiver<LspEvent>,
    stop: Arc<AtomicBool>,
    next_id: Arc<std::sync::atomic::AtomicI64>,
    pending: Pending,
    supervisor: Option<std::thread::JoinHandle<()>>,
}

impl LspHandle {
    /// Discover, spawn and hand back a handle — or say why not, in a sentence.
    ///
    /// **Must be called from a thread that outlives the server.** `cide-app` calls it on the
    /// long-lived `cide-spawn` thread via `child_env::on_spawn_thread`, which is the whole reason
    /// that function exists. See the crate docs.
    pub fn start(server: Server, roots: Vec<PathBuf>) -> Result<Self, LspError> {
        let binary = match crate::discover::find(server, &roots) {
            Found::Ready(path) => path,
            Found::Missing(reason) => return Err(LspError::Unavailable(reason)),
        };

        let (outbox_tx, outbox_rx) = crossbeam_channel::bounded::<Value>(OUTBOX);
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<LspEvent>();
        let stop = Arc::new(AtomicBool::new(false));
        let pending: Pending = Arc::new(parking_lot::Mutex::new(HashMap::new()));

        let supervisor = {
            let stop = Arc::clone(&stop);
            let roots = roots.clone();
            let pending = Arc::clone(&pending);
            std::thread::Builder::new()
                .name(format!("cide-lsp-{}", server.binary()))
                .spawn(move || supervise(server, binary, roots, outbox_rx, event_tx, stop, pending))
                .map_err(|error| LspError::Spawn {
                    binary: server.binary().to_string(),
                    message: error.to_string(),
                })?
        };

        Ok(Self {
            server,
            outbox: outbox_tx,
            events: event_rx,
            stop,
            next_id: Arc::new(std::sync::atomic::AtomicI64::new(REQUEST_ID_BASE)),
            pending,
            supervisor: Some(supervisor),
        })
    }

    /// A cloneable sender for request/response traffic.
    ///
    /// Take one, drop whatever lock you hold, *then* wait. See [`Requester`].
    pub fn requester(&self) -> Requester {
        Requester {
            server: self.server,
            outbox: self.outbox.clone(),
            next_id: Arc::clone(&self.next_id),
            pending: Arc::clone(&self.pending),
        }
    }

    pub fn server(&self) -> Server {
        self.server
    }

    /// Everything the server has said since the last call. Never blocks.
    pub fn drain(&self) -> Vec<LspEvent> {
        self.events.try_iter().collect()
    }

    /// Queue one message. Dropped with a line if the server has stopped reading — see [`OUTBOX`].
    pub fn send(&self, message: Value) {
        match self.outbox.try_send(message) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                tracing::warn!(
                    server = self.server.binary(),
                    "outbound queue full; dropping a message"
                );
            }
            Err(TrySendError::Disconnected(_)) => {
                tracing::debug!(server = self.server.binary(), "server is gone");
            }
        }
    }
}

impl Drop for LspHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Before the join, not after: the join waits out the shutdown ladder, and a caller blocked
        // on a request would otherwise sit through it and then be told `Timeout` — a wrong reason
        // for a server that is being stopped deliberately.
        cancel_pending(&self.pending, RequestError::ServerGone);
        // Dropping the sender is what wakes a supervisor blocked on `recv_timeout`.
        let (dead, _) = crossbeam_channel::bounded(0);
        let _ = std::mem::replace(&mut self.outbox, dead);
        if let Some(handle) = self.supervisor.take() {
            // Joined rather than detached: the ladder writes `shutdown`/`exit` and waits, and a
            // detached supervisor would let the process exit while a `gopls` was still flushing
            // the cache the ladder exists to preserve.
            let _ = handle.join();
        }
    }
}

/// Why one life of a server ended.
struct Failure {
    /// The tail of its stderr, or whatever else we can say.
    reason: String,
    /// Did it ever answer `initialize`? See [`supervise`] for what turns on this.
    ever_handshook: bool,
}

/// The sentence for a server that never started.
///
/// Leads with the server's own stderr when there is any, because that line is almost always the
/// whole answer — and appends the one diagnosis a user cannot be expected to make themselves.
fn start_failure_reason(server: Server, stderr: &str) -> String {
    let mut reason = format!("{} could not start.", server.binary());
    if !stderr.is_empty() {
        reason.push(' ');
        reason.push_str(stderr);
    }
    // The rustup shim. `~/.cargo/bin/rust-analyzer` is a symlink to `rustup`, so it exists and is
    // executable whatever the toolchain actually has — and it answers with this. Recognised by
    // the message rather than by inspecting the symlink, because the same failure reaches us
    // through `rustup run`, a wrapper script, and a `.cargo/bin` copied between machines.
    if stderr.contains("Unknown binary") || stderr.contains("no such command") {
        reason.push_str(
            " That is rustup's shim answering, not the server: the component is not installed. \
             Run `rustup component add rust-analyzer`.",
        );
    }
    reason
}

/// The supervisor thread: spawn, pump, restart, give up.
fn supervise(
    server: Server,
    binary: PathBuf,
    roots: Vec<PathBuf>,
    outbox: Receiver<Value>,
    events: Sender<LspEvent>,
    stop: Arc<AtomicBool>,
    pending: Pending,
) {
    supervise_lives(server, binary, roots, &outbox, &events, &stop, &pending);
    /*
     * The last word on every waiter, wherever the supervisor exited.
     *
     * Here rather than at each `return` inside, and that is the whole point: an earlier version
     * cancelled only around the `run_once` call, which read as complete and was not. The stop
     * check at the top of the loop returns *before* `run_once` is ever entered, so a request
     * registered in that window was never released — its caller sat out the full five-second
     * deadline and was then told the server "did not answer in time", for a server that had been
     * deliberately stopped. Wrapping the whole function is what makes the guarantee survive the
     * next `return` somebody adds.
     *
     * Waiters registered after this line cannot exist: the `Receiver` is dropped when this
     * function returns, so `try_send` in `Requester::request` fails with `Disconnected` and the
     * caller is told `ServerGone` without ever entering the map.
     */
    cancel_pending(&pending, RequestError::ServerGone);
}

fn supervise_lives(
    server: Server,
    binary: PathBuf,
    roots: Vec<PathBuf>,
    outbox: &Receiver<Value>,
    events: &Sender<LspEvent>,
    stop: &Arc<AtomicBool>,
    pending: &Pending,
) {
    let mut crashes: Vec<Instant> = Vec::new();

    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }

        // Every exit inside `run_once` ends this life, so the waiters go here — once, around the
        // call, rather than at each of its four exits, which is how one of them gets missed.
        // `supervise` above repeats it for the paths that never reach this line at all.
        let outcome = run_once(server, &binary, &roots, outbox, events, stop, pending);
        cancel_pending(pending, RequestError::ServerGone);
        match outcome {
            Ok(()) => return,
            // Died before answering `initialize`. Not a crash — a **failure to start**, and no
            // amount of retrying fixes one: the binary is missing a component, was built for
            // another libc, or is a shim that cannot dispatch. Retrying costs seven seconds and
            // then buries the one useful line (the server's own stderr) under a sentence about
            // crash counts.
            //
            // Found by running this against a real machine: `~/.cargo/bin/rust-analyzer` is
            // often a **symlink to rustup**, which passes every "is it on PATH and executable"
            // probe and then exits with `Unknown binary 'rust-analyzer' in official toolchain`.
            // That sentence is what the user needs, immediately and on its own.
            Err(Failure {
                reason,
                ever_handshook: false,
            }) => {
                let _ = events.send(LspEvent::Status(SourceStatus::Unavailable {
                    reason: start_failure_reason(server, &reason),
                }));
                return;
            }
            Err(Failure { reason, .. }) => {
                if stop.load(Ordering::Acquire) {
                    return;
                }
                crashes.retain(|at| at.elapsed() < CRASH_WINDOW);
                crashes.push(Instant::now());
                if crashes.len() >= BACKOFF.len() {
                    let _ = events.send(LspEvent::Status(SourceStatus::Unavailable {
                        reason: format!(
                            "{} exited {} times in five minutes and was not restarted. \
                             Its last output is in the log (Help ▸ Open log folder). {reason}",
                            server.binary(),
                            crashes.len(),
                        ),
                    }));
                    return;
                }
                let wait = BACKOFF[crashes.len() - 1];
                tracing::warn!(
                    server = server.binary(),
                    %reason,
                    "restarting in {wait:?}",
                );
                let _ = events.send(LspEvent::Status(SourceStatus::Scanning {
                    detail: format!("restarting {}", server.binary()),
                }));
                /*
                 * Wait out the backoff, discarding anything queued for the session that just
                 * died — and releasing anyone waiting on it.
                 *
                 * Two bugs lived in the three lines this replaces, and both were invisible:
                 *
                 * * **A discarded message could be a request.** The waiter had been registered
                 *   before the send, the cancellation for this life had already run above, and
                 *   the next life is a fresh child that was never told to answer. The caller sat
                 *   out its own deadline and was told "still indexing" about a server that had
                 *   crashed — the exact `Timeout`-versus-`ServerGone` confusion `RequestError`
                 *   exists to keep apart. So every discard is followed by a cancellation.
                 * * **One queued message skipped the rest of the backoff.** The old form was
                 *   `if recv_timeout(wait).is_ok() || stop { }` with an empty body, which fell
                 *   straight back into the loop and respawned. An editor sending `didChange`
                 *   every 300 ms therefore defeated the backoff entirely and turned a crash loop
                 *   into a respawn storm — which is what the backoff is for.
                 *
                 * Polled in short slices rather than one long `recv_timeout` so a shutdown is
                 * still noticed at once instead of after four seconds.
                 */
                let deadline = Instant::now() + wait;
                while Instant::now() < deadline {
                    if stop.load(Ordering::Acquire) {
                        return;
                    }
                    match outbox.recv_timeout(BACKOFF_TICK) {
                        Ok(_) => cancel_pending(pending, RequestError::ServerGone),
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                        // The handle was dropped: nothing more is coming and nothing is waiting.
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
                    }
                }
            }
        }
    }
}

/// One life of one server. `Ok(())` means an orderly stop; `Err` means it died.
fn run_once(
    server: Server,
    binary: &PathBuf,
    roots: &[PathBuf],
    outbox: &Receiver<Value>,
    events: &Sender<LspEvent>,
    stop: &AtomicBool,
    pending: &Pending,
) -> Result<(), Failure> {
    let mut command = Command::new(binary);
    command
        .current_dir(roots.first().cloned().unwrap_or_else(std::env::temp_dir))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Both spawn rules, and neither is optional — see the crate docs. `scrub_command` first
    // because `arm` installs a `pre_exec` hook and reads nothing from the environment.
    cide_core::child_env::scrub_command(&mut command);
    cide_core::child_env::arm(&mut command);

    let mut child: Child = command.spawn().map_err(|e| Failure {
        reason: e.to_string(),
        ever_handshook: false,
    })?;
    let mut stdin = child.stdin.take().ok_or_else(|| Failure {
        reason: "no stdin".into(),
        ever_handshook: false,
    })?;
    let stdout = child.stdout.take().ok_or_else(|| Failure {
        reason: "no stdout".into(),
        ever_handshook: false,
    })?;
    let stderr = child.stderr.take().ok_or_else(|| Failure {
        reason: "no stderr".into(),
        ever_handshook: false,
    })?;

    let (inbound_tx, inbound_rx) = crossbeam_channel::unbounded::<Value>();
    let tail = Arc::new(parking_lot::Mutex::new(String::new()));

    // Reader. Ends on EOF, which is what a server exiting looks like.
    let reader = std::thread::Builder::new()
        .name(format!("cide-lsp-{}-rx", server.binary()))
        .spawn(move || {
            let mut input = BufReader::new(stdout);
            while let Ok(message) = codec::read_message(&mut input) {
                if inbound_tx.send(message).is_err() {
                    return;
                }
            }
        })
        .map_err(|e| Failure {
            reason: e.to_string(),
            ever_handshook: false,
        })?;

    // stderr drain. Never parsed — a server's stderr is human prose — but the tail of it is the
    // only evidence a crash report can carry.
    let stderr_thread = {
        let tail = Arc::clone(&tail);
        std::thread::Builder::new()
            .name(format!("cide-lsp-{}-err", server.binary()))
            .spawn(move || {
                let mut buf = [0u8; 4096];
                let mut stderr = stderr;
                while let Ok(read) = stderr.read(&mut buf) {
                    if read == 0 {
                        return;
                    }
                    let text = String::from_utf8_lossy(&buf[..read]);
                    tracing::debug!(target: "cide::lsp", "{}", text.trim_end());
                    let mut tail = tail.lock();
                    tail.push_str(&text);
                    if tail.len() > KEEP_STDERR {
                        let cut = tail.len() - KEEP_STDERR;
                        // On a char boundary, or `String::drain` panics on a multi-byte tail.
                        let cut = (0..=cut)
                            .rev()
                            .find(|i| tail.is_char_boundary(*i))
                            .unwrap_or(0);
                        tail.drain(..cut);
                    }
                }
            })
            .map_err(|e| Failure {
                reason: e.to_string(),
                ever_handshook: false,
            })?
    };

    let (mut session, initial) = Session::new(roots, server.binary());
    // When a held-back `Ready` becomes believable. See `READY_SETTLE`.
    let mut ready_at: Option<Instant> = None;
    let write = |stdin: &mut std::process::ChildStdin,
                 effects: Vec<Effect>,
                 ready_at: &mut Option<Instant>|
     -> Result<(), String> {
        for effect in effects {
            match effect {
                Effect::Send(value) => {
                    codec::write_message(stdin, &value).map_err(|e| e.to_string())?;
                }
                Effect::Status(status) => {
                    if matches!(status, SourceStatus::Ready) {
                        // Held, not sent. The loop below releases it once it has survived
                        // `READY_SETTLE` without a `Scanning` overtaking it.
                        *ready_at = Some(Instant::now() + READY_SETTLE);
                    } else {
                        // Anything else lands immediately *and* cancels a pending `Ready` — a
                        // server that went back to work was never ready.
                        *ready_at = None;
                        let _ = events.send(LspEvent::Status(status));
                    }
                }
                Effect::Publish { abs_path, items } => {
                    let _ = events.send(LspEvent::Published { abs_path, items });
                }
            }
        }
        Ok(())
    };
    // The `initialize` write, and the first place a shim that cannot dispatch shows itself: the
    // child is already gone, so this is a broken pipe rather than anything about the protocol.
    write(&mut stdin, initial, &mut ready_at).map_err(|reason| Failure {
        reason,
        ever_handshook: false,
    })?;

    // The pump. Two channels and a timeout, so neither side can starve the other and a stop is
    // noticed within the tick even when the server has gone silent.
    // `write` yields a plain `String`; the loop converts at each break, where it also knows
    // whether the handshake had completed.
    let outcome = loop {
        if stop.load(Ordering::Acquire) {
            break Ok(());
        }
        crossbeam_channel::select! {
            recv(inbound_rx) -> message => match message {
                Ok(message) => {
                    // A reply to a caller's request, resolved *before* `Session` sees it.
                    //
                    // `Session::on_message` returns an empty effect vec for any response whose id
                    // is not `initialize_id` — it parses the reply and throws it away, with no log
                    // line anywhere. Intercepting here is what makes a request possible at all;
                    // the alternative (a new `Effect`/`LspEvent` variant) cannot work, because
                    // `drain()` has exactly one consumer and a command reaching into it to find
                    // its own reply would swallow the `Published` events the diagnostics pump
                    // needed.
                    if take_reply(pending, &message) {
                        continue;
                    }
                    let effects = session.on_message(&message);
                    if let Err(reason) = write(&mut stdin, effects, &mut ready_at) {
                        break Err(Failure { reason, ever_handshook: session.ever_handshook() });
                    }
                }
                // The reader ended: the server closed stdout, i.e. it exited.
                Err(_) => break Err(Failure {
                    reason: tail.lock().trim().to_string(),
                    ever_handshook: session.ever_handshook(),
                }),
            },
            recv(outbox) -> message => match message {
                Ok(message) => {
                    if let Err(error) = codec::write_message(&mut stdin, &message) {
                        break Err(Failure {
                            reason: error.to_string(),
                            ever_handshook: session.ever_handshook(),
                        });
                    }
                }
                // The handle was dropped.
                Err(_) => break Ok(()),
            },
            // Short enough that a settled `Ready` is released promptly rather than waiting on
            // the next message — which, for a server that has genuinely finished, may never come.
            default(Duration::from_millis(100)) => {}
        }

        // Release a `Ready` that has held for long enough.
        if let Some(due) = ready_at
            && Instant::now() >= due
        {
            ready_at = None;
            let _ = events.send(LspEvent::Status(SourceStatus::Ready));
        }
    };

    // The ladder. `shutdown` then `exit` first, always: gopls writes its cache on `exit`, and a
    // signal first costs the user that cache and makes the next start re-index from nothing.
    let _ = write(&mut stdin, session.shutdown(), &mut ready_at);
    drop(stdin);

    let deadline = Instant::now() + GRACE;
    let mut exited = false;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => {
                exited = true;
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(_) => break,
        }
    }
    if !exited {
        terminate(&child);
        let deadline = Instant::now() + KILL_AFTER;
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                exited = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        if !exited {
            let _ = child.kill();
        }
    }
    let _ = child.wait();
    let _ = reader.join();
    let _ = stderr_thread.join();

    outcome
}

/// `SIGTERM`, the polite rung. `Child::kill` is `SIGKILL` and skips it.
#[cfg(unix)]
fn terminate(child: &Child) {
    // SAFETY: a bare syscall with a pid we own and a constant signal number.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
}

#[cfg(not(unix))]
fn terminate(_child: &Child) {}

#[cfg(test)]
mod tests {
    /// The request path, driven without a child process.
    ///
    /// Everything below exercises `take_reply` + the pending map directly, because the interesting
    /// failures are all correlation failures and none of them needs a real server to reproduce.
    mod requests {
        use super::*;
        use serde_json::json;

        fn parts() -> (Requester, Pending, Receiver<Value>) {
            let (tx, rx) = crossbeam_channel::bounded::<Value>(OUTBOX);
            let pending: Pending = Arc::new(parking_lot::Mutex::new(HashMap::new()));
            let requester = Requester {
                server: Server::RustAnalyzer,
                outbox: tx,
                next_id: Arc::new(std::sync::atomic::AtomicI64::new(REQUEST_ID_BASE)),
                pending: Arc::clone(&pending),
            };
            (requester, pending, rx)
        }

        #[test]
        fn a_request_id_is_never_the_handshakes_or_the_shutdowns() {
            // `Session::new` re-issues 1 and 2 on every life of every server. An allocator that
            // could hand out either would have a caller's request resolved by the handshake reply.
            let (requester, _pending, rx) = parts();
            for _ in 0..5 {
                let requester = requester.clone();
                std::thread::spawn(move || {
                    let _ = requester.request("test/x", json!({}), Duration::from_millis(1));
                });
            }
            let mut seen = Vec::new();
            while seen.len() < 5 {
                let message = rx.recv_timeout(Duration::from_secs(5)).expect("written");
                seen.push(message["id"].as_i64().expect("an id"));
            }
            seen.sort_unstable();
            assert!(
                seen.iter().all(|id| *id >= REQUEST_ID_BASE),
                "ids: {seen:?}"
            );
            seen.dedup();
            assert_eq!(seen.len(), 5, "ids were reused across concurrent callers");
        }

        #[test]
        fn a_reply_reaches_the_waiter_that_asked() {
            let (requester, pending, rx) = parts();
            let waiter = {
                let requester = requester.clone();
                std::thread::spawn(move || {
                    requester.request("textDocument/definition", json!({}), Duration::from_secs(5))
                })
            };
            let asked = rx.recv_timeout(Duration::from_secs(5)).expect("written");
            let id = asked["id"].as_i64().expect("an id");
            assert!(take_reply(
                &pending,
                &json!({"id": id, "result": {"ok": true}})
            ));
            assert_eq!(
                waiter.join().expect("joined").expect("answered")["ok"],
                json!(true)
            );
        }

        #[test]
        fn a_server_to_client_request_is_not_mistaken_for_a_reply() {
            // `workspace/configuration` carries an id *and* a method. Swallowing it here would
            // leave rust-analyzer waiting for an answer for ever — the stall this whole crate's
            // handshake notes warn about, arriving through the new code path.
            let (_requester, pending, _rx) = parts();
            let message = json!({"id": 1, "method": "workspace/configuration", "params": {}});
            assert!(
                !take_reply(&pending, &message),
                "it must fall through to Session"
            );
        }

        #[test]
        fn a_notification_is_not_mistaken_for_a_reply() {
            let (_requester, pending, _rx) = parts();
            let message = json!({"method": "textDocument/publishDiagnostics", "params": {}});
            assert!(!take_reply(&pending, &message));
        }

        #[test]
        fn an_error_response_becomes_the_servers_own_message() {
            let (requester, pending, rx) = parts();
            let waiter = {
                let requester = requester.clone();
                std::thread::spawn(move || {
                    requester.request("test/x", json!({}), Duration::from_secs(5))
                })
            };
            let id = rx.recv_timeout(Duration::from_secs(5)).expect("written")["id"]
                .as_i64()
                .expect("an id");
            take_reply(
                &pending,
                &json!({"id": id, "error": {"code": -32603, "message": "boom"}}),
            );
            assert_eq!(
                waiter.join().expect("joined"),
                Err(RequestError::Failed("boom".to_string()))
            );
        }

        #[test]
        fn a_null_result_is_an_answer_and_not_an_error() {
            // `null` is what a server says for "I could not resolve this", and it is the *common*
            // reply. Turning it into an error would make every miss look like a broken server.
            let (requester, pending, rx) = parts();
            let waiter = {
                let requester = requester.clone();
                std::thread::spawn(move || {
                    requester.request("test/x", json!({}), Duration::from_secs(5))
                })
            };
            let id = rx.recv_timeout(Duration::from_secs(5)).expect("written")["id"]
                .as_i64()
                .expect("an id");
            take_reply(&pending, &json!({"id": id, "result": Value::Null}));
            assert_eq!(waiter.join().expect("joined"), Ok(Value::Null));
        }

        #[test]
        fn a_deadline_that_passes_is_a_timeout_and_leaves_no_entry_behind() {
            let (requester, pending, _rx) = parts();
            assert_eq!(
                requester.request("test/x", json!({}), Duration::from_millis(50)),
                Err(RequestError::Timeout)
            );
            assert!(
                pending.lock().is_empty(),
                "a timed-out waiter leaked its slot"
            );
        }

        #[test]
        fn cancelling_releases_every_waiter_rather_than_letting_them_time_out() {
            // The path that runs when a server dies: a caller must hear `ServerGone` at once, not
            // sit out its own deadline and then report the wrong reason.
            let (requester, pending, rx) = parts();
            let waiter = {
                let requester = requester.clone();
                std::thread::spawn(move || {
                    requester.request("test/x", json!({}), Duration::from_secs(30))
                })
            };
            let _ = rx.recv_timeout(Duration::from_secs(5)).expect("written");
            let started = Instant::now();
            cancel_pending(&pending, RequestError::ServerGone);
            assert_eq!(
                waiter.join().expect("joined"),
                Err(RequestError::ServerGone)
            );
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "it waited out its deadline"
            );
            assert!(pending.lock().is_empty());
        }

        #[test]
        fn a_full_outbox_is_reported_rather_than_dropped() {
            // `LspHandle::send` drops in this situation with a warn, which is right for a
            // notification nobody awaits and wrong for a request somebody does.
            let (requester, pending, _rx) = parts();
            for _ in 0..OUTBOX {
                requester.outbox.try_send(json!({})).expect("filled");
            }
            assert_eq!(
                requester.request("test/x", json!({}), Duration::from_secs(1)),
                Err(RequestError::Queue)
            );
            assert!(
                pending.lock().is_empty(),
                "a refused request leaked its slot"
            );
        }

        #[test]
        fn a_waiter_is_released_when_the_supervisor_exits_without_running_a_life() {
            // The stop-at-the-top path. `supervise` cancels around the *whole* function precisely
            // because this `return` happens before `run_once` is ever entered — an earlier version
            // cancelled only around the call and left this caller to sit out its own deadline and
            // then be told "still indexing" about a server that was deliberately stopped.
            let (_tx, rx) = crossbeam_channel::bounded::<Value>(OUTBOX);
            let (events, _events_rx) = crossbeam_channel::unbounded::<LspEvent>();
            let pending: Pending = Arc::new(parking_lot::Mutex::new(HashMap::new()));
            let (waiter_tx, waiter_rx) = crossbeam_channel::bounded(1);
            pending.lock().insert(REQUEST_ID_BASE, waiter_tx);

            // Stop already set: `supervise` must return through the top-of-loop check.
            let stop = Arc::new(AtomicBool::new(true));
            supervise(
                Server::RustAnalyzer,
                PathBuf::from("/nonexistent/never-spawned"),
                Vec::new(),
                rx,
                events,
                stop,
                Arc::clone(&pending),
            );

            assert_eq!(
                waiter_rx.try_recv(),
                Ok(Err(RequestError::ServerGone)),
                "the waiter was left to time out on a server that never ran"
            );
            assert!(pending.lock().is_empty());
        }

        #[test]
        fn a_duplicate_reply_cannot_resolve_a_later_request() {
            // A misbehaving server answering twice must not hand the second answer to whoever
            // happens to hold that id next.
            let (requester, pending, rx) = parts();
            let waiter = {
                let requester = requester.clone();
                std::thread::spawn(move || {
                    requester.request("test/x", json!({}), Duration::from_secs(5))
                })
            };
            let id = rx.recv_timeout(Duration::from_secs(5)).expect("written")["id"]
                .as_i64()
                .expect("an id");
            assert!(take_reply(&pending, &json!({"id": id, "result": 1})));
            assert!(
                !take_reply(&pending, &json!({"id": id, "result": 2})),
                "resolved twice"
            );
            assert_eq!(waiter.join().expect("joined"), Ok(json!(1)));
        }
    }

    use super::*;

    #[test]
    fn a_server_that_is_not_installed_reports_a_sentence_rather_than_spawning() {
        // The name is deliberately one no machine has. The failure must be `Unavailable` with
        // prose, not a spawn error with an errno.
        let outcome = crate::discover::find(
            // A `Server` variant cannot be invented, so this asserts the shape through the real
            // path: no roots means no project marker, which is the second probe.
            Server::RustAnalyzer,
            &[],
        );
        assert!(matches!(outcome, Found::Missing(_)));
    }

    #[test]
    fn the_backoff_table_and_the_give_up_rule_agree() {
        // `crashes.len() >= BACKOFF.len()` is the give-up test, so the table's length *is* the
        // number of restarts. Pinned because changing one without the other silently either
        // restarts for ever or never restarts at all.
        assert_eq!(BACKOFF.len(), 3);
        assert!(BACKOFF.windows(2).all(|w| w[0] < w[1]), "not increasing");
        assert!(CRASH_WINDOW > BACKOFF.iter().sum::<Duration>());
    }

    #[test]
    fn a_rustup_shim_is_diagnosed_rather_than_reported_as_three_crashes() {
        // Found by running this crate against a real machine. `~/.cargo/bin/rust-analyzer` is a
        // **symlink to rustup**, so it passes every "on PATH and executable" probe and then exits
        // immediately with the message below. Before this, the user got "rust-analyzer exited 3
        // times in five minutes" after seven seconds of pointless backoff, with the one actionable
        // line buried at the end of it.
        let reason = start_failure_reason(
            Server::RustAnalyzer,
            "error: Unknown binary 'rust-analyzer' in official toolchain '1.92.0-x86_64-unknown-linux-gnu'.",
        );
        assert!(reason.contains("could not start"), "{reason}");
        // The server's own words first — they are almost always the whole answer.
        assert!(reason.contains("Unknown binary"), "{reason}");
        // Then the one diagnosis a user cannot be expected to make themselves.
        assert!(
            reason.contains("rustup component add rust-analyzer"),
            "{reason}"
        );
    }

    #[test]
    fn an_ordinary_start_failure_is_reported_without_the_rustup_guess() {
        // The diagnosis is appended only when the evidence is there. Attaching it to every
        // failure would send users to fix a component that was never the problem.
        let reason = start_failure_reason(Server::Gopls, "fork/exec: permission denied");
        assert!(reason.contains("permission denied"), "{reason}");
        assert!(!reason.contains("rustup"), "{reason}");
    }

    #[test]
    fn a_start_failure_with_no_stderr_still_says_something() {
        let reason = start_failure_reason(Server::Gopls, "");
        assert!(reason.contains("gopls could not start"), "{reason}");
    }

    #[test]
    fn a_ready_is_held_long_enough_to_bridge_a_gap_between_progress_tokens() {
        // The constants behind the fix for the flapping `Ready`. Measured against the real
        // rust-analyzer on this workspace: eight Ready→Scanning flips in the first 0.9 s, every
        // gap ≤ 250 ms, true settle at 14.9 s.
        //
        // A unit test cannot drive a real server, so what is pinned here is the *relationship*:
        // the hold must comfortably exceed the observed gaps, and the pump must wake often enough
        // to release it — a settle shorter than the poll would never fire for a server that has
        // finished and gone silent.
        assert!(
            READY_SETTLE >= Duration::from_millis(500),
            "too short to bridge the gaps between rust-analyzer's progress tokens"
        );
        assert!(
            READY_SETTLE <= Duration::from_secs(3),
            "a finished server should be reported promptly, not after a pause the user notices"
        );
        assert!(
            READY_SETTLE > Duration::from_millis(100),
            "the pump's idle tick must be able to release a settled Ready"
        );
    }

    #[test]
    fn the_grace_period_leaves_room_for_the_ladder() {
        // `shutdown` reply, then exit, then SIGTERM, then SIGKILL. If `GRACE` were shorter than a
        // slow server's flush, the ladder would degenerate into "signal immediately".
        assert!(GRACE >= Duration::from_secs(1));
        assert!(KILL_AFTER <= GRACE);
    }
}
