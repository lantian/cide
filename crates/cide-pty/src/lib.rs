//! PTY-backed sessions.
//!
//! # Why threads and not tokio
//!
//! `portable-pty`'s reader and writer are blocking `std::io` handles. Wrapping them in
//! `spawn_blocking` would burn a runtime thread per session anyway, so we own the threads
//! explicitly and keep tokio for the MCP WebSocket server, where it earns its place.
//!
//! # The pipeline
//!
//! ```text
//! child ─write()→ kernel PTY buffer ─→ reader thread (64 KiB blocking reads)
//!                                        │  bounded crossbeam channel, cap 16
//!                                        ↓
//!                                   coalescer thread ──→ vt100::Parser (always)
//!                                        │                └─ the reattach mirror
//!                                        └──→ Sinks (attached webviews)
//! ```
//!
//! Backpressure is structural rather than negotiated: when the bounded channel fills, the
//! reader blocks, the kernel PTY buffer fills, and the child blocks in `write()`. A
//! `cat bigfile` throttles itself exactly as it would in a native terminal, and the UI
//! cannot be flooded into unresponsiveness.
//!
//! # Coalescing is a correctness requirement, not a tuning knob
//!
//! Tauri routes raw `Channel` payloads below `MAX_RAW_DIRECT_EXECUTE_THRESHOLD` (1024
//! bytes) through `webview.eval` with the bytes spelled out as a JSON array of decimal
//! numbers — roughly 5x inflation, dispatched on the GTK main loop. An unbatched
//! interactive PTY yields 20-200 byte reads, so without coalescing every single frame
//! takes that path. We therefore flush on [`FLUSH_BYTES`] **or** [`FLUSH_INTERVAL`],
//! whichever comes first, capped at [`MAX_FRAME`].

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded, unbounded};
use parking_lot::Mutex;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Flush as soon as this many bytes have accumulated.
///
/// Chosen to sit comfortably above Tauri's 1024-byte raw threshold so frames always take
/// the fast custom-protocol path.
pub const FLUSH_BYTES: usize = 8 * 1024;

/// Flush a partial buffer after this long, so an idle prompt still appears promptly.
pub const FLUSH_INTERVAL: Duration = Duration::from_millis(8);

/// Never emit a single frame larger than this.
pub const MAX_FRAME: usize = 64 * 1024;

/// Depth of the reader→coalescer channel. Small on purpose: this is the backpressure.
const READ_QUEUE_DEPTH: usize = 16;

/// How long [`PtySession::attach_with_snapshot`] waits for the coalescer to cut the stream.
///
/// Generous, because the only thing that can delay the answer is a flush already in progress —
/// bounded by one `state_formatted()` over the scrollback — and finite because this runs on the
/// thread that receives IPC messages. Expiring costs the attach its atomicity, not the attach.
const ATTACH_TIMEOUT: Duration = Duration::from_secs(2);

/// How much a sink may owe before it stops being sent raw output, and how it recovers.
///
/// # Why credit is per-sink and not per-session
///
/// A session can have several sinks — a mirrored pane, a pane mid-detach. If credit were a
/// session-wide counter, one webview that stopped acking would stall the session for every
/// other pane watching it, and the user would see two healthy panes freeze because a third
/// one they had forgotten about was occluded. Each sink owes for itself.
///
/// # Why a choked sink is skipped rather than blocked on
///
/// The bounded reader channel already throttles the *child*; that is the right pressure for
/// a genuinely fast producer. But a wedged webview is not a fast producer — blocking the
/// coalescer on it would freeze the agent's terminal because a GPU context was lost. So a
/// choked sink is skipped and owes a catch-up frame instead. The `vt100` mirror is fed
/// unconditionally, so what it missed is always reconstructible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CreditPolicy {
    /// Outstanding bytes at which a sink stops receiving raw output.
    pub high: usize,
    /// Outstanding bytes at which a choked sink resumes. Below [`Self::high`], so a sink
    /// that is merely keeping pace does not flap between the two states every frame.
    pub low: usize,
    /// A choked sink that has not acked for this long is assumed wedged — crashed, GPU
    /// hang, occluded-window throttle, WebGL context loss — and has its credit force-reset.
    ///
    /// Without this a session pins above `high` for ever and looks frozen with no error
    /// anywhere, which is the failure mode this whole mechanism exists to prevent.
    pub watchdog: Duration,
}

impl Default for CreditPolicy {
    fn default() -> Self {
        Self {
            high: 256 * 1024,
            low: 32 * 1024,
            watchdog: Duration::from_secs(5),
        }
    }
}

/// Size of each blocking read from the PTY master.
const READ_BUF: usize = 64 * 1024;

/// Reported when the child was reaped but its status could not be read.
///
/// `child.wait()` returning an error is the only way to get here — the pid was already
/// reaped by something else, or the syscall failed. -1 rather than 0 because a fabricated
/// success is the one value that could later be mistaken for a real one, and the shell's
/// status space is 0..=255 so nothing legitimate collides with it.
pub const UNKNOWN_EXIT_CODE: i32 = -1;

/// How a child ended.
///
/// # Why the code is not just `ExitStatus::exit_code()`
///
/// `portable-pty` converts a `std::process::ExitStatus` into its own type and, for a child
/// that died of a signal, throws the signal *number* away and keeps a strsignal *name* —
/// filling `code` with a flat 1. That makes an OOM kill (SIGKILL) indistinguishable from
/// `exit 1`, and indistinguishable from the SIGTERM the app itself sends on quit, which is
/// exactly the distinction anyone reading an exit code wants. So [`signal_number`] maps the
/// name back and this carries the shell's `128 + signum` instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exit {
    /// The shell convention: the child's own status, or `128 + signum` when a signal ended
    /// it, or [`UNKNOWN_EXIT_CODE`] when the status could not be read at all.
    pub code: i32,
    /// The signal's name — `"Killed"`, `"Terminated"` — when a signal ended the child.
    ///
    /// Kept beside the code because the code is a number a log reader has to decode and
    /// this is the sentence they were looking for. `None` for an ordinary exit.
    pub signal: Option<String>,
}

impl Exit {
    /// Whether this is the ordinary "it finished, and it worked" case.
    pub fn is_success(&self) -> bool {
        self.code == 0
    }
}

/// Called once, on the reaper thread, when the child has been reaped.
type ExitCallback = Box<dyn FnOnce(Exit) + Send + 'static>;

/// The reaper's answer, and whoever is waiting on it.
///
/// One slot behind one lock rather than a channel, because there are two arrival orders and
/// both are ordinary: a watcher registered while the child was still running has to be
/// parked, and one registered after it died has to be answered immediately. A channel makes
/// the second case a message with no receiver — which is precisely the bug the polling
/// version in `cide-app` existed to work around.
enum ExitSlot {
    Running(Vec<ExitCallback>),
    Reaped(Exit),
}

/// Lines of scrollback the vt100 mirror retains for reattach.
const SCROLLBACK: usize = 5_000;

#[derive(Debug, thiserror::Error)]
pub enum PtyError {
    #[error("failed to open a pty: {0}")]
    OpenPty(#[source] anyhow_compat::Error),
    #[error("failed to spawn {program}: {source}")]
    Spawn {
        program: String,
        #[source]
        source: anyhow_compat::Error,
    },
    #[error("failed to take the pty writer: {0}")]
    TakeWriter(#[source] anyhow_compat::Error),
    #[error("failed to clone the pty reader: {0}")]
    CloneReader(#[source] anyhow_compat::Error),
    #[error("failed to resize the pty: {0}")]
    Resize(#[source] anyhow_compat::Error),
}

/// `portable-pty` returns `anyhow::Error`; this alias keeps that dependency from leaking
/// into our public API surface as a hard requirement on a specific anyhow version.
pub mod anyhow_compat {
    pub type Error = Box<dyn std::error::Error + Send + Sync + 'static>;
}

/// Terminal geometry, in cells and in pixels.
///
/// Pixel dimensions must be real. Passing zeroes is the common shortcut and it breaks any
/// program that queries the cell size for sixel/image output or for mouse pixel reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl Geometry {
    pub fn new(cols: u16, rows: u16, cell_width: u16, cell_height: u16) -> Self {
        Self {
            cols: cols.max(1),
            rows: rows.max(1),
            cell_width,
            cell_height,
        }
    }

    fn to_pty_size(self) -> PtySize {
        PtySize {
            rows: self.rows,
            cols: self.cols,
            pixel_width: self.cell_width.saturating_mul(self.cols),
            pixel_height: self.cell_height.saturating_mul(self.rows),
        }
    }
}

impl Default for Geometry {
    fn default() -> Self {
        Self::new(80, 24, 8, 17)
    }
}

/// What to run, where, and with which environment.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Applied after the inherited environment.
    pub env: Vec<(String, String)>,
    /// Removed from the inherited environment before `env` is applied.
    ///
    /// `TMUX` in particular is not optional to scrub: its presence makes Claude Code clamp
    /// to 256 colours unconditionally, which visibly desaturates the accent colour the
    /// whole design is built on.
    pub env_remove: Vec<String>,
    pub geometry: Geometry,
    /// Flow-control thresholds. Overridden in tests, which cannot wait five seconds to
    /// watch a watchdog fire.
    pub credit: CreditPolicy,
    /// Bytes fed to the screen mirror before the child produces anything.
    ///
    /// **Into the mirror only — never into the child, and never into the pty.** This is how
    /// a restored shell gets its previous screen back: the caller hands over the bytes
    /// `screen_state()` produced in the last run, and the new session's `vt100::Parser`
    /// starts already holding them, so the first `attach_with_snapshot` replays them exactly
    /// the way it replays a re-dock.
    ///
    /// The alternative that lost was replaying in the frontend — one `term.write` before
    /// hydration. It puts the text in xterm and nowhere else, so the moment that host is
    /// evicted and rehydrated (which re-reads the mirror, by design) the replayed screen
    /// vanishes and the pane silently loses history it had a second ago. Seeding the mirror
    /// keeps one source of truth for what a pane shows.
    ///
    /// Whatever is in here is *dead text*: it was produced by a process that no longer
    /// exists. Saying so is the caller's job — see `cide_app::lifecycle::restore_notice`.
    pub preload: Vec<u8>,
}

impl SpawnSpec {
    pub fn new(program: impl Into<String>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            env: Vec::new(),
            env_remove: Vec::new(),
            geometry: Geometry::default(),
            credit: CreditPolicy::default(),
            preload: Vec::new(),
        }
    }

    pub fn arg(mut self, a: impl Into<String>) -> Self {
        self.args.push(a.into());
        self
    }

    pub fn env(mut self, k: impl Into<String>, v: impl Into<String>) -> Self {
        self.env.push((k.into(), v.into()));
        self
    }

    pub fn env_remove(mut self, k: impl Into<String>) -> Self {
        self.env_remove.push(k.into());
        self
    }

    pub fn geometry(mut self, g: Geometry) -> Self {
        self.geometry = g;
        self
    }

    pub fn credit(mut self, c: CreditPolicy) -> Self {
        self.credit = c;
        self
    }

    /// Seed the screen mirror with bytes from a previous run. See [`SpawnSpec::preload`].
    pub fn preload(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.preload = bytes.into();
        self
    }
}

/// A consumer of session output.
///
/// Implemented over a Tauri `Channel` in the app crate, and over a plain channel in tests
/// and in `cide-headless`. Returning `false` means the sink is gone and should be dropped.
pub trait Sink: Send + Sync + 'static {
    fn deliver(&self, bytes: &[u8]) -> bool;
}

impl<F> Sink for F
where
    F: Fn(&[u8]) -> bool + Send + Sync + 'static,
{
    fn deliver(&self, bytes: &[u8]) -> bool {
        self(bytes)
    }
}

/// Identifies one attachment so it can be detached without disturbing the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SinkId(u64);

struct Registered {
    id: SinkId,
    sink: Arc<dyn Sink>,
    /// Bytes delivered to this sink that it has not yet acknowledged.
    outstanding: AtomicUsize,
    /// Hysteresis state. Separate from comparing `outstanding` to a threshold so a sink
    /// hovering around the mark does not alternate between choked and clear every frame.
    choked: AtomicBool,
    /// Set when the sink was skipped, so the next frame it receives is the current screen
    /// rather than a fragment resuming mid-escape-sequence.
    missed: AtomicBool,
    /// When credit last moved, for the watchdog.
    last_ack: Mutex<Instant>,
}

/// Work handed to the coalescer thread that is not output.
///
/// Exists for exactly one reason: an attachment has to happen at a point in the stream where
/// the mirror and the sinks agree, and the coalescer thread is the only place such a point
/// exists. See [`PtySession::attach_with_snapshot`].
enum Control {
    Attach {
        id: SinkId,
        sink: Arc<dyn Sink>,
        /// The screen as of the cut point. The caller sends this to the sink itself.
        reply: Sender<Vec<u8>>,
    },
}

/// What the coalescer sends to sinks.
enum Frame {
    Bytes(Vec<u8>),
    /// No new output — re-evaluate credit and pay out any owed catch-up.
    ///
    /// Without this, unchoking would only ever happen on the next frame of real output. A
    /// sink that choked during a burst and then went quiet — which is precisely what a
    /// finished `cat` looks like — would sit on stale content indefinitely, waiting for a
    /// frame that is not coming.
    Tick,
    Eof,
}

/// A live PTY session.
///
/// Owned by the session registry in the app crate, never by a window, tab or pane —
/// closing any of those detaches a sink, it does not kill the child.
pub struct PtySession {
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer_tx: Sender<Vec<u8>>,
    /// Attach requests, serviced on the coalescer thread. A channel of its own rather than a
    /// variant on the reader→coalescer channel: that one reaching `Disconnected` is how EOF is
    /// detected, and holding a second sender for it here would mean the coalescer never sees
    /// the child go away.
    control_tx: Sender<Control>,
    vt: Arc<Mutex<vt100::Parser>>,
    sinks: Arc<Mutex<Vec<Registered>>>,
    next_sink_id: AtomicU32,
    geometry: Mutex<Geometry>,
    child_pid: Option<u32>,
    exited: Arc<AtomicBool>,
    /// The reaper's answer. Separate from `exited` on purpose: `exited` is *also* set by the
    /// coalescer when the master reaches EOF, which can happen before — or, if a descendant
    /// is still holding the pty open, after — the child is actually reaped. `has_exited()`
    /// answers "is this pane dead" as early as possible, which is what the `— exited —`
    /// marker and the credit watchdog want; this answers "and how", which only the reaper
    /// knows.
    exit: Arc<Mutex<ExitSlot>>,
    killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
    credit: CreditPolicy,
}

impl PtySession {
    /// Open a PTY, spawn the child, and start the reader, writer and coalescer threads.
    pub fn spawn(spec: SpawnSpec) -> Result<Arc<Self>, PtyError> {
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(spec.geometry.to_pty_size())
            .map_err(|e| PtyError::OpenPty(e.into()))?;

        let mut cmd = CommandBuilder::new(&spec.program);
        for a in &spec.args {
            cmd.arg(a);
        }
        cmd.cwd(&spec.cwd);
        for k in &spec.env_remove {
            cmd.env_remove(k);
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }

        let child = pair.slave.spawn_command(cmd).map_err(|e| PtyError::Spawn {
            program: spec.program.clone(),
            source: e.into(),
        })?;
        let child_pid = child.process_id();

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| PtyError::TakeWriter(e.into()))?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| PtyError::CloneReader(e.into()))?;

        // Dropping the slave is not tidiness — it is what makes EOF work. While this
        // process still holds a slave fd, the master never sees the child's exit, the
        // reader thread blocks forever, and the pane shows a live cursor on a dead
        // process. M2's "kill the child, see `— exited —`" check exists to prove this
        // line is present.
        drop(pair.slave);

        let vt = Arc::new(Mutex::new(vt100::Parser::new(
            spec.geometry.rows,
            spec.geometry.cols,
            SCROLLBACK,
        )));
        // Before any sink can exist and before the coalescer thread starts, so the preload
        // cannot interleave with the child's own first bytes. It is fed to the parser rather
        // than delivered to sinks because there are none yet — every consumer picks it up
        // through `attach_with_snapshot`, on the ordinary re-dock path.
        if !spec.preload.is_empty() {
            vt.lock().process(&spec.preload);
        }
        let sinks: Arc<Mutex<Vec<Registered>>> = Arc::new(Mutex::new(Vec::new()));
        let exited = Arc::new(AtomicBool::new(false));

        let (raw_tx, raw_rx) = bounded::<Vec<u8>>(READ_QUEUE_DEPTH);
        let (writer_tx, writer_rx) = unbounded::<Vec<u8>>();
        let (control_tx, control_rx) = unbounded::<Control>();

        spawn_reader(reader, raw_tx);
        spawn_coalescer(
            raw_rx,
            control_rx,
            Arc::clone(&vt),
            Arc::clone(&sinks),
            Arc::clone(&exited),
            spec.credit,
        );
        spawn_writer(writer, writer_rx);

        let killer = child.clone_killer();
        let exit = Arc::new(Mutex::new(ExitSlot::Running(Vec::new())));
        spawn_reaper(child, Arc::clone(&exited), Arc::clone(&exit));

        Ok(Arc::new(Self {
            master: Mutex::new(pair.master),
            writer_tx,
            control_tx,
            vt,
            sinks,
            next_sink_id: AtomicU32::new(1),
            geometry: Mutex::new(spec.geometry),
            child_pid,
            exited,
            exit,
            killer: Mutex::new(killer),
            credit: spec.credit,
        }))
    }

    /// The child's OS process id.
    ///
    /// This is the join key for the IDE MCP server: `claude` sends its pid in the
    /// `ide_connected` notification, so one WebSocket connection maps to exactly one pane.
    pub fn child_pid(&self) -> Option<u32> {
        self.child_pid
    }

    /// Whether this pane's child is gone, as early as anything here can tell.
    ///
    /// Set by whichever of the two observers gets there first: the coalescer, when the pty
    /// master reaches EOF, or the reaper, when `wait()` returns. Deliberately *not*
    /// tightened to "has been reaped" — the `— exited —` marker and the shutdown ladder
    /// both want the earliest honest answer, and [`Self::exit_status`] is where the precise
    /// one lives.
    pub fn has_exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    /// How the child ended, or `None` while it is still running or not yet reaped.
    ///
    /// `has_exited()` can be true while this is still `None`: EOF on the master and the
    /// reaper's `wait()` are two different events on two different threads. A caller that
    /// needs the status should use [`Self::on_exit`] rather than poll this, which is here
    /// for tests and the debug overlay.
    pub fn exit_status(&self) -> Option<Exit> {
        match &*self.exit.lock() {
            ExitSlot::Running(_) => None,
            ExitSlot::Reaped(exit) => Some(exit.clone()),
        }
    }

    /// Call `f` once, with the real exit status, when the child is reaped.
    ///
    /// Runs on the reaper thread, so `f` must not block for long — it is what makes the
    /// pty threads' shutdown observable. If the child has *already* been reaped, `f` runs
    /// immediately on the calling thread instead of never: registration racing the child's
    /// death is ordinary (a one-shot `claude --version` can be gone before `spawn` has
    /// returned), and a watcher that silently missed its event is how a pane ends up
    /// showing a live cursor on a dead process.
    ///
    /// The alternative that lost was polling `has_exited()` on a thread per session, which
    /// is what `cide-app` did before this existed: it cost a wakeup every 200 ms per pane
    /// and, worse, could only ever report that the child was gone — never how, because by
    /// the time it noticed, `waitpid` had already been called and answers `ECHILD`.
    pub fn on_exit(&self, f: impl FnOnce(Exit) + Send + 'static) {
        let mut slot = self.exit.lock();
        let exit = match &mut *slot {
            ExitSlot::Running(waiting) => {
                waiting.push(Box::new(f));
                return;
            }
            ExitSlot::Reaped(exit) => exit.clone(),
        };
        // Never with the lock held: `f` is arbitrary caller code and may well ask this
        // session something.
        drop(slot);
        f(exit);
    }

    pub fn geometry(&self) -> Geometry {
        *self.geometry.lock()
    }

    /// The flow-control thresholds this session was spawned with.
    pub fn credit_policy(&self) -> CreditPolicy {
        self.credit
    }

    /// Attach a consumer.
    ///
    /// Sinks are a **list**, not a single slot. That makes detach gapless — the new
    /// window's sink is registered before the old one is dropped, so no byte falls
    /// between them — and it makes mirroring a session into a second pane fall out for
    /// free rather than being a feature.
    ///
    /// The caller is responsible for first delivering [`Self::screen_state`] so the new
    /// consumer starts from the current screen rather than mid-stream.
    pub fn attach(&self, sink: Arc<dyn Sink>) -> SinkId {
        let id = self.mint_sink_id();
        self.sinks.lock().push(registered(id, sink));
        id
    }

    /// Attach a consumer **and** take the screen it should start from, at one cut point.
    ///
    /// # The bug this replaces
    ///
    /// `screen_state()` then `attach()` is two operations, and there is a window between them
    /// in which bytes are counted twice. The mirror is fed per *chunk*, the moment the
    /// coalescer receives it; sinks are fed per *flush*, up to [`FLUSH_INTERVAL`] later. So a
    /// byte that arrived during that window is already painted into the snapshot and is still
    /// sitting in the coalescer's `pending` buffer, which the new sink is then sent in full.
    /// The pane shows the tail of its own scrollback twice. For a fullscreen TUI, whose repaint
    /// the frontend nudges immediately afterwards, it is invisible; for a shell pane it is a
    /// duplicated prompt and a duplicated last command, on every re-dock and every rehydration.
    ///
    /// # Why it goes to the coalescer thread
    ///
    /// Because that thread is the only place where "the mirror and the sinks are in step" is
    /// ever true. It flushes `pending` first — so the existing sinks receive those bytes
    /// exactly once, as ordinary output — then registers the new sink, then renders the
    /// mirror. Nothing runs between the three, because they are three statements on the one
    /// thread that broadcasts.
    ///
    /// The alternative that lost was moving `vt.process` from the receive arm into `broadcast`,
    /// so the mirror only ever advances when the sinks do. It removes the window too, but it
    /// makes the mirror lag the child by up to a flush interval — and `session_scrollback` on a
    /// *quiet* session would then answer with a screen missing the bytes that arrived since
    /// the last flush. That converts a visible duplication into a silent loss, which is worse.
    ///
    /// Falls back to the unsynchronised pair if the coalescer is gone, which is the case for a
    /// child that has already exited: there is no more output, so there is no window to be
    /// wrong about.
    pub fn attach_with_snapshot(&self, sink: Arc<dyn Sink>) -> (SinkId, Vec<u8>) {
        let id = self.mint_sink_id();

        if !self.exited.load(Ordering::Acquire) {
            let (reply_tx, reply_rx) = bounded::<Vec<u8>>(1);
            let request = Control::Attach {
                id,
                sink: Arc::clone(&sink),
                reply: reply_tx,
            };
            if self.control_tx.send(request).is_ok()
                // Bounded rather than `recv()`: this runs on the thread that receives IPC
                // messages, and a coalescer that has exited between the check above and the
                // send would otherwise wedge the whole webview. A timeout that expires means
                // the fallback below, not a lost attachment.
                && let Ok(screen) = reply_rx.recv_timeout(ATTACH_TIMEOUT)
            {
                return (id, screen);
            }
        }

        self.sinks.lock().push(registered(id, sink));
        (id, self.screen_state())
    }

    fn mint_sink_id(&self) -> SinkId {
        SinkId(self.next_sink_id.fetch_add(1, Ordering::Relaxed) as u64)
    }

    /// Detaching drops the sink's credit with it, rather than stranding it.
    ///
    /// This is why the detach race needs no separate handling: a pane that goes away
    /// mid-flight cannot leave behind an unpayable debt that chokes the session for ever.
    pub fn detach(&self, id: SinkId) {
        self.sinks.lock().retain(|r| r.id != id);
    }

    pub fn sink_count(&self) -> usize {
        self.sinks.lock().len()
    }

    /// A consumer reports that it has finished processing `bytes` of output.
    ///
    /// The frontend calls this from `term.write`'s completion callback — the only point at
    /// which xterm has actually parsed the bytes rather than merely received them.
    pub fn ack(&self, id: SinkId, bytes: usize) {
        let sinks = self.sinks.lock();
        let Some(r) = sinks.iter().find(|r| r.id == id) else {
            return;
        };
        // Saturating, because an ack for a frame delivered before a watchdog reset would
        // otherwise wrap the counter to usize::MAX and choke the sink permanently — the
        // exact deadlock this mechanism exists to avoid, reintroduced by arithmetic.
        let _ = r
            .outstanding
            .fetch_update(Ordering::Release, Ordering::Acquire, |cur| {
                Some(cur.saturating_sub(bytes))
            });
        *r.last_ack.lock() = Instant::now();
    }

    /// Bytes delivered to a sink but not yet acked. For tests and the debug overlay.
    pub fn outstanding(&self, id: SinkId) -> usize {
        self.sinks
            .lock()
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.outstanding.load(Ordering::Acquire))
            .unwrap_or(0)
    }

    /// Whether a sink is currently being skipped for owing too much.
    pub fn is_choked(&self, id: SinkId) -> bool {
        self.sinks
            .lock()
            .iter()
            .find(|r| r.id == id)
            .is_some_and(|r| r.choked.load(Ordering::Acquire))
    }

    /// A byte sequence that reconstructs the current screen on a fresh terminal:
    /// contents, cursor, alt-screen flag, bracketed paste and mouse protocol modes.
    ///
    /// This is the reattach primitive. It is a *screen model*, not a byte-exact recorder —
    /// OSC 8 hyperlinks, OSC 52 clipboard traffic and DEC 2026 sync framing are not
    /// modelled. For a fullscreen TUI the caller should follow it with a one-frame
    /// `cols-1 → cols` resize nudge so the application repaints from its own state.
    pub fn screen_state(&self) -> Vec<u8> {
        self.vt.lock().screen().state_formatted()
    }

    /// Whether the child currently has the alternate screen engaged, i.e. it is a
    /// fullscreen TUI and reattach should nudge it into repainting.
    pub fn in_alternate_screen(&self) -> bool {
        self.vt.lock().screen().alternate_screen()
    }

    /// Queue bytes for the child.
    ///
    /// Never blocks: a multi-megabyte paste goes onto an unbounded queue serviced by the
    /// writer thread, so it cannot stall the IPC command that submitted it.
    pub fn write(&self, bytes: Vec<u8>) {
        let _ = self.writer_tx.send(bytes);
    }

    /// Resize the PTY (which raises SIGWINCH in the child) and the screen mirror.
    pub fn resize(&self, geometry: Geometry) -> Result<(), PtyError> {
        self.master
            .lock()
            .resize(geometry.to_pty_size())
            .map_err(|e| PtyError::Resize(e.into()))?;
        self.vt
            .lock()
            .screen_mut()
            .set_size(geometry.rows, geometry.cols);
        *self.geometry.lock() = geometry;
        Ok(())
    }

    /// Ask the child to exit. Escalation to SIGKILL is the caller's policy.
    pub fn kill(&self) {
        let _ = self.killer.lock().kill();
    }
}

fn spawn_reader(mut reader: Box<dyn Read + Send>, tx: Sender<Vec<u8>>) {
    thread::Builder::new()
        .name("cide-pty-read".into())
        .spawn(move || {
            let mut buf = vec![0u8; READ_BUF];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        // A blocking send here is the whole backpressure design: it stalls
                        // this thread, which stops draining the kernel PTY buffer, which
                        // blocks the child in write().
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        })
        .expect("spawn pty reader thread");
}

/// One turn of the coalescer's loop, so the `select!` has a single value to match on.
enum Event {
    Output(Vec<u8>),
    Control(Control),
    /// Nothing arrived within the flush deadline: flush and service credit.
    Idle,
    /// Either channel closed. The child is gone.
    Eof,
}

/// A fresh registration, owing nothing.
///
/// A function rather than a `Registered::new`, because the struct is private and this is the
/// only shape it is ever built in — the two call sites (`attach` and the coalescer's control
/// arm) differing would be a credit bug that shows up as one pane choking and not the other.
fn registered(id: SinkId, sink: Arc<dyn Sink>) -> Registered {
    Registered {
        id,
        sink,
        outstanding: AtomicUsize::new(0),
        choked: AtomicBool::new(false),
        missed: AtomicBool::new(false),
        last_ack: Mutex::new(Instant::now()),
    }
}

/// Handle one control request, on the coalescer thread.
///
/// The order is the whole point and it is not interchangeable:
///
/// 1. **Flush** what is pending, to the sinks that already exist. Those bytes are in the
///    mirror already, so leaving them queued would send them to the new sink as live output
///    *after* it had received them inside the snapshot.
/// 2. **Register** the new sink, which from here on receives only future frames.
/// 3. **Render** the mirror, which now contains exactly what step 1 delivered and nothing
///    more.
///
/// Nothing can run between them: this thread is the only one that broadcasts, and the mirror
/// only advances from this thread's output arm.
fn serve_control(
    request: Control,
    sinks: &Arc<Mutex<Vec<Registered>>>,
    vt: &Arc<Mutex<vt100::Parser>>,
    policy: &CreditPolicy,
    pending: &mut Vec<u8>,
    first_byte_at: &mut Option<Instant>,
) {
    match request {
        Control::Attach { id, sink, reply } => {
            if !pending.is_empty() {
                broadcast(sinks, vt, policy, Frame::Bytes(std::mem::take(pending)));
                *first_byte_at = None;
            }
            sinks.lock().push(registered(id, sink));
            // A caller that has given up (see `ATTACH_TIMEOUT`) leaves nobody on the other
            // end. The sink stays attached regardless — it is registered and will receive
            // output; only the atomicity of its first frame was lost.
            let _ = reply.send(vt.lock().screen().state_formatted());
        }
    }
}

fn spawn_coalescer(
    rx: Receiver<Vec<u8>>,
    control_rx: Receiver<Control>,
    vt: Arc<Mutex<vt100::Parser>>,
    sinks: Arc<Mutex<Vec<Registered>>>,
    exited: Arc<AtomicBool>,
    policy: CreditPolicy,
) {
    thread::Builder::new()
        .name("cide-pty-coalesce".into())
        .spawn(move || {
            let mut pending: Vec<u8> = Vec::with_capacity(FLUSH_BYTES * 2);
            let mut first_byte_at: Option<Instant> = None;

            loop {
                // Wait indefinitely when idle; once bytes are pending, only wait out the
                // remainder of the flush interval.
                let timeout = match first_byte_at {
                    None => Duration::from_millis(250),
                    Some(t) => FLUSH_INTERVAL.saturating_sub(t.elapsed()),
                };

                // Selected on rather than polled between reads: an idle session waits 250 ms
                // per iteration, and an attach that queued behind that wait would put a
                // quarter-second stall in front of every pane opening onto a quiet shell.
                //
                // `control_rx` cannot disconnect while this thread lives — the session holds
                // the sender, and if it did not, `Err` on this arm would spin the loop. That
                // is asserted by construction rather than handled: a disconnected control arm
                // ends the thread, exactly as a disconnected output arm does.
                let event = crossbeam_channel::select! {
                    recv(control_rx) -> request => match request {
                        Ok(request) => Event::Control(request),
                        Err(_) => Event::Eof,
                    },
                    recv(rx) -> chunk => match chunk {
                        Ok(chunk) => Event::Output(chunk),
                        Err(_) => Event::Eof,
                    },
                    default(timeout) => Event::Idle,
                };

                match event {
                    Event::Control(request) => {
                        serve_control(
                            request,
                            &sinks,
                            &vt,
                            &policy,
                            &mut pending,
                            &mut first_byte_at,
                        );
                    }
                    Event::Output(chunk) => {
                        // The mirror is fed unconditionally, attached or not — that is what
                        // makes a reattaching pane able to paint the current screen.
                        vt.lock().process(&chunk);
                        if first_byte_at.is_none() {
                            first_byte_at = Some(Instant::now());
                        }
                        pending.extend_from_slice(&chunk);

                        while pending.len() >= MAX_FRAME {
                            let rest = pending.split_off(MAX_FRAME);
                            broadcast(
                                &sinks,
                                &vt,
                                &policy,
                                Frame::Bytes(std::mem::replace(&mut pending, rest)),
                            );
                        }
                        if pending.len() >= FLUSH_BYTES {
                            broadcast(
                                &sinks,
                                &vt,
                                &policy,
                                Frame::Bytes(std::mem::take(&mut pending)),
                            );
                            first_byte_at = None;
                        }
                    }
                    Event::Idle => {
                        if !pending.is_empty() {
                            broadcast(
                                &sinks,
                                &vt,
                                &policy,
                                Frame::Bytes(std::mem::take(&mut pending)),
                            );
                            first_byte_at = None;
                        }
                        // Service credit even with nothing to send, so a sink that choked
                        // during a burst still recovers once the burst ends.
                        broadcast(&sinks, &vt, &policy, Frame::Tick);
                    }
                    Event::Eof => {
                        if !pending.is_empty() {
                            broadcast(
                                &sinks,
                                &vt,
                                &policy,
                                Frame::Bytes(std::mem::take(&mut pending)),
                            );
                        }
                        exited.store(true, Ordering::Release);
                        broadcast(&sinks, &vt, &policy, Frame::Eof);
                        break;
                    }
                }
            }
        })
        .expect("spawn pty coalescer thread");
}

/// Deliver one frame, applying per-sink credit.
///
/// Three states per sink, resolved in this order:
///
/// 1. **Clear** — send the raw bytes and charge them.
/// 2. **Choked** — skip, and note that the sink now owes a catch-up. If it has been choked
///    past the watchdog with no ack it is assumed wedged: reset its credit and let it
///    through, because a permanently silent pane is worse than a pane that skipped ahead.
/// 3. **Owing a catch-up** — send the current screen instead of the raw frame. Resuming
///    with raw bytes would splice the sink back in mid-escape-sequence and corrupt its
///    rendering; the screen mirror is a complete, self-consistent starting point.
fn broadcast(
    sinks: &Arc<Mutex<Vec<Registered>>>,
    vt: &Arc<Mutex<vt100::Parser>>,
    policy: &CreditPolicy,
    frame: Frame,
) {
    let (bytes, tick) = match frame {
        Frame::Bytes(b) => (b, false),
        Frame::Tick => (Vec::new(), true),
        // EOF is signalled out of band by the session's `exited` flag; sinks that care
        // observe it through the session handle rather than through a sentinel byte
        // sequence that could collide with real output.
        Frame::Eof => return,
    };

    let now = Instant::now();
    // Rendered at most once per frame and only if some sink actually needs it —
    // `state_formatted()` walks the whole screen, which is not worth doing for the
    // overwhelmingly common case where every sink is keeping up.
    let mut catchup: Option<Vec<u8>> = None;

    let mut guard = sinks.lock();
    guard.retain(|r| {
        let outstanding = r.outstanding.load(Ordering::Acquire);

        if r.choked.load(Ordering::Acquire) {
            if outstanding <= policy.low {
                r.choked.store(false, Ordering::Release);
            } else if now.duration_since(*r.last_ack.lock()) >= policy.watchdog {
                // Wedged. Forgive the debt rather than let it pin the sink for ever.
                r.outstanding.store(0, Ordering::Release);
                r.choked.store(false, Ordering::Release);
                *r.last_ack.lock() = now;
            } else {
                r.missed.store(true, Ordering::Release);
                return true;
            }
        } else if outstanding >= policy.high {
            r.choked.store(true, Ordering::Release);
            r.missed.store(true, Ordering::Release);
            return true;
        }

        // A tick carries no output, so a sink that is up to date has nothing to receive.
        if tick && !r.missed.load(Ordering::Acquire) {
            return true;
        }

        let payload: &[u8] = if r.missed.swap(false, Ordering::AcqRel) {
            catchup.get_or_insert_with(|| vt.lock().screen().state_formatted())
        } else {
            &bytes
        };

        r.outstanding.fetch_add(payload.len(), Ordering::Release);
        r.sink.deliver(payload)
    });
}

fn spawn_writer(mut writer: Box<dyn Write + Send>, rx: Receiver<Vec<u8>>) {
    thread::Builder::new()
        .name("cide-pty-write".into())
        .spawn(move || {
            while let Ok(bytes) = rx.recv() {
                if writer.write_all(&bytes).is_err() {
                    break;
                }
                let _ = writer.flush();
            }
        })
        .expect("spawn pty writer thread");
}

/// Reap the child and publish how it went.
///
/// This thread is the only place the exit status exists. `wait()` consumes it — a second
/// `waitpid` on that pid answers `ECHILD` — so a status dropped here is gone for good,
/// which is why the previous version could only ever report -1.
fn spawn_reaper(
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    exited: Arc<AtomicBool>,
    slot: Arc<Mutex<ExitSlot>>,
) {
    thread::Builder::new()
        .name("cide-pty-reap".into())
        .spawn(move || {
            let exit = classify(child.wait());
            // Three steps, and the order is the whole point.
            //
            // 1. Store the status, so `has_exited()` can never flip ahead of
            //    `exit_status()` — a caller that sees the flag go true because of *this*
            //    thread can already read how it went.
            // 2. Set the flag, which is what the shutdown ladder is polling.
            // 3. Only then run the watchers, which are arbitrary caller code.
            //
            // Running the watchers before step 2 was the first version of this, and it
            // makes "is this child dead yet" hostage to whatever a watcher does —
            // `cide-app`'s emits a Tauri event. A watcher that panicked would unwind this
            // thread with `exited` still false for a child that is provably gone, and
            // `stop_children` would then wait out every rung of the ladder on a corpse.
            let waiting = settle(&slot, &exit);
            exited.store(true, Ordering::Release);
            for callback in waiting {
                callback(exit.clone());
            }
        })
        .expect("spawn pty reaper thread");
}

/// Turn what `wait()` returned into the status we report.
fn classify(status: std::io::Result<portable_pty::ExitStatus>) -> Exit {
    let Ok(status) = status else {
        return Exit {
            code: UNKNOWN_EXIT_CODE,
            signal: None,
        };
    };
    match status.signal() {
        Some(name) => Exit {
            // `exit_code()` is a flat 1 for every signalled child, so falling back to it
            // when the name will not map is a last resort rather than the answer.
            code: signal_number(name).map_or_else(|| status.exit_code() as i32, |n| 128 + n),
            signal: Some(name.to_string()),
        },
        None => Exit {
            code: status.exit_code() as i32,
            signal: None,
        },
    }
}

/// Publish `exit` into the slot and hand back everyone who was waiting on it.
///
/// Deliberately does *not* call the watchers itself. They are arbitrary caller code and the
/// caller has one more thing to do first — see [`spawn_reaper`] — so returning them keeps
/// the ordering decision in the one place that can see all of it. Calling them here would
/// also have to be outside the lock anyway: a watcher that asks this session anything would
/// otherwise deadlock against `on_exit`.
fn settle(slot: &Mutex<ExitSlot>, exit: &Exit) -> Vec<ExitCallback> {
    let mut guard = slot.lock();
    match std::mem::replace(&mut *guard, ExitSlot::Reaped(exit.clone())) {
        ExitSlot::Running(waiting) => waiting,
        // Unreachable: one reaper thread per session, and it settles once. Restoring the
        // first answer rather than trusting the second is the conservative choice if
        // that ever stops being true — the first reap is the real one.
        ExitSlot::Reaped(first) => {
            *guard = ExitSlot::Reaped(first);
            Vec::new()
        }
    }
}

/// The number behind a strsignal name, so `128 + n` can be reported.
///
/// A reverse lookup rather than a guess: `portable-pty` produced the name by calling
/// `strsignal` in *this* process, so asking the same libc in the same locale for the same
/// strings gets the same table back. A hardcoded English list would be wrong for any user
/// whose `LC_MESSAGES` is not English — and wrong silently, reporting `exit 1` for an OOM
/// kill, which is the failure this whole change exists to remove.
///
/// `None` when nothing matches, which the caller reports as the flat status rather than as
/// an invented signal.
#[cfg(unix)]
fn signal_number(name: &str) -> Option<i32> {
    // `portable-pty`'s own fallback spelling when `strsignal` returns null, and the one case
    // the table scan below cannot answer: a name libc would not produce is a name libc will
    // not match either.
    //
    // Falls *through* rather than returning on a name that merely begins this way without
    // carrying a number — a `return … .ok()` here would skip the table entirely for any
    // locale whose real signal names happen to start "Signal …". The range check is not
    // decoration either: the caller computes `128 + n` from this, and `"Signal 2147483647"`
    // would overflow that and panic in a debug build.
    if let Some(rest) = name.strip_prefix("Signal ")
        && let Ok(n) = rest.trim().parse::<i32>()
        && (1..=MAX_SIGNAL).contains(&n)
    {
        return Some(n);
    }
    // Scanning stops at the first hit and only ever runs once per session.
    (1..=MAX_SIGNAL).find(|&n| signal_name(n).as_deref() == Some(name))
}

/// The highest signal number this will believe in.
///
/// 64 covers every standard signal plus the whole real-time range on Linux. It is a bound on
/// what `128 + n` may be built from as much as it is a scan limit.
#[cfg(unix)]
const MAX_SIGNAL: i32 = 64;

#[cfg(not(unix))]
fn signal_number(_name: &str) -> Option<i32> {
    None
}

/// What libc calls signal `n`, or `None` if it will not say.
#[cfg(unix)]
fn signal_name(n: i32) -> Option<String> {
    // SAFETY: `strsignal` takes an int and returns a pointer to a string libc owns — a
    // static for a known signal, a per-thread buffer for an unknown one. We copy it out
    // before returning and never free it, and never hold it across another libc call.
    let ptr = unsafe { libc::strsignal(n) };
    if ptr.is_null() {
        return None;
    }
    // SAFETY: non-null and NUL-terminated by the contract of `strsignal`.
    let name = unsafe { std::ffi::CStr::from_ptr(ptr) };
    Some(name.to_string_lossy().into_owned())
}

// `TrySendError` is re-exported for callers that want a non-blocking write path later.
pub use crossbeam_channel::TrySendError as WriteQueueFull;
const _: fn(TrySendError<Vec<u8>>) = |_| {};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// How long a test waits before calling a condition unmet.
    ///
    /// Deliberately generous. Every test here exits the moment its condition holds, so the
    /// deadline only bounds *failure* and costs nothing when things work. Five seconds was
    /// enough in isolation and not enough beside a parallel `cargo test` with a compile
    /// running, which made `reports_exit_after_the_child_finishes` fail once and pass on the
    /// next three runs — a flaky test being strictly worse than no test.
    const DEADLINE: Duration = Duration::from_secs(30);

    fn collect_sink() -> (Arc<dyn Sink>, mpsc::Receiver<Vec<u8>>) {
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let tx = Mutex::new(tx);
        let sink: Arc<dyn Sink> =
            Arc::new(move |bytes: &[u8]| tx.lock().send(bytes.to_vec()).is_ok());
        (sink, rx)
    }

    /// Collect frames until `needle` appears, or the deadline passes.
    ///
    /// Waiting for the content rather than for the clock. Draining until a fixed duration
    /// elapses makes every run cost the full deadline even when the bytes arrived in
    /// milliseconds — the reason this suite took thirty seconds to tell us everything was
    /// fine. The deadline now bounds only the failing case, which is what a deadline is for.
    fn drain_until(rx: &mpsc::Receiver<Vec<u8>>, needle: &str, dur: Duration) -> Vec<u8> {
        let deadline = Instant::now() + dur;
        let mut out = Vec::new();
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(remaining) {
                Ok(chunk) => {
                    out.extend_from_slice(&chunk);
                    if String::from_utf8_lossy(&out).contains(needle) {
                        return out;
                    }
                }
                // The sender is gone, or nothing more is coming; whatever arrived is the
                // whole answer and the caller's assertion decides whether it is enough.
                Err(_) => break,
            }
        }
        out
    }

    #[test]
    fn echoes_child_output_to_an_attached_sink() {
        // Driven through stdin rather than `sh -c 'printf ...'`. A one-shot child can finish
        // and take the coalescer down with it before `attach` ever runs, which made this
        // test fail intermittently against a session that was behaving correctly. Asking an
        // interactive shell for the output *after* attaching removes the race instead of
        // papering over it with a sleep.
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir());
        let session = PtySession::spawn(spec).expect("spawn sh");
        let (sink, rx) = collect_sink();
        session.attach(sink);

        // The needle is computed by the shell, so finding it proves the command ran rather
        // than merely that the terminal echoed what we typed.
        session.write(b"echo $((6*7))-answer\n".to_vec());

        let out = drain_until(&rx, "42-answer", DEADLINE);
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("42-answer"), "got: {text:?}");
        session.kill();
    }

    #[test]
    fn output_produced_before_a_sink_attaches_survives_in_the_mirror() {
        // Why the race above is a test artefact and not a product bug: the vt100 mirror is
        // fed unconditionally, attached or not, so a pane that attaches late still has a
        // complete screen to paint. This is the same guarantee detach/re-dock relies on.
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("printf 'printed-before-anyone-was-listening'");
        let session = PtySession::spawn(spec).expect("spawn sh");

        // Waited for on the *mirror*, not on the process.
        //
        // The first version polled `has_exited()` and then read the screen once, which is a
        // race this test lost under load: `exited` is set by the reaper the moment `wait()`
        // returns, and the reaper is a different thread from the coalescer that feeds the
        // mirror. The child can be gone while its last bytes are still in the channel. It
        // passed for months on timing alone.
        //
        // Waiting for the content is both correct and stricter: it cannot pass early, and
        // the deadline now bounds only the failing case.
        let deadline = Instant::now() + DEADLINE;
        let mut screen = String::new();
        while Instant::now() < deadline {
            screen = String::from_utf8_lossy(&session.screen_state()).into_owned();
            if screen.contains("printed-before-anyone-was-listening") {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        // Nothing was ever attached; the output still exists.
        assert!(
            screen.contains("printed-before-anyone-was-listening"),
            "the mirror lost output produced with no sink attached: {screen:?}"
        );
    }

    #[test]
    fn reports_exit_after_the_child_finishes() {
        // This is the regression guard for `drop(pair.slave)`. Without that drop the
        // master never observes EOF and this test hangs until the timeout.
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("exit 0");
        let session = PtySession::spawn(spec).expect("spawn sh");

        let deadline = Instant::now() + DEADLINE;
        while !session.has_exited() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(session.has_exited(), "child exit was never observed");
    }

    /// Block until the child has been reaped, or give up. Returns the status.
    ///
    /// `has_exited()` is not the condition: it flips on EOF too, and EOF is a different
    /// thread's event. Waiting on the *status* is what these tests are actually about.
    fn await_exit(session: &PtySession) -> Exit {
        let deadline = Instant::now() + DEADLINE;
        while Instant::now() < deadline {
            if let Some(exit) = session.exit_status() {
                return exit;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("the child was never reaped");
    }

    #[test]
    fn an_ordinary_exit_code_is_reported_verbatim() {
        // The whole point of threading the status out: 7 has to arrive as 7, not as the
        // flat -1 the app used to publish for every dead session.
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("exit 7");
        let session = PtySession::spawn(spec).expect("spawn sh");

        let exit = await_exit(&session);
        assert_eq!(exit.code, 7, "the child's own status was not reported");
        assert_eq!(exit.signal, None, "nothing signalled this child");
        assert!(!exit.is_success());
    }

    #[test]
    fn a_clean_exit_is_zero_and_carries_no_signal() {
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("exit 0");
        let session = PtySession::spawn(spec).expect("spawn sh");

        let exit = await_exit(&session);
        assert_eq!(exit.code, 0);
        assert!(exit.is_success());
    }

    #[cfg(unix)]
    #[test]
    fn a_killed_child_is_distinguishable_from_one_that_failed() {
        // The case the whole change exists for. `claude` killed by the OOM reaper and
        // `claude` exiting 1 are the same event to anyone reading `portable-pty`'s status,
        // which fills `exit_code` with a flat 1 for *every* signalled child. 137 vs 1 is
        // the difference between "your machine ran out of memory" and "it reported an
        // error", and the user only ever sees the number.
        let killed = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("kill -KILL $$");
        let killed = PtySession::spawn(killed).expect("spawn sh");
        let killed = await_exit(&killed);

        let failed = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("exit 1");
        let failed = PtySession::spawn(failed).expect("spawn sh");
        let failed = await_exit(&failed);

        assert_eq!(killed.code, 128 + libc::SIGKILL, "SIGKILL must read as 137");
        assert!(
            killed.signal.is_some(),
            "a signalled child must name its signal: {killed:?}"
        );
        assert_eq!(failed.code, 1);
        assert_eq!(failed.signal, None);
        assert_ne!(
            killed.code, failed.code,
            "an OOM kill and `exit 1` reported the same thing"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_terminated_child_is_distinguishable_from_a_killed_one() {
        // The app's own shutdown ladder sends SIGHUP then SIGTERM, so "the user quit" has
        // to read differently from "something killed it".
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("kill -TERM $$");
        let session = PtySession::spawn(spec).expect("spawn sh");

        let exit = await_exit(&session);
        assert_eq!(exit.code, 128 + libc::SIGTERM);
    }

    #[cfg(unix)]
    #[test]
    fn every_signal_name_libc_gives_us_maps_back_to_its_number() {
        // `signal_number` is a reverse lookup over exactly the table `portable-pty` used to
        // build the name, so a round trip has to be the identity. Checked across the whole
        // range rather than for SIGKILL alone, because the failure mode is one name that
        // collides or comes back reworded under a different locale.
        let mut checked = 0;
        for n in 1..=31 {
            let Some(name) = signal_name(n) else { continue };
            checked += 1;
            assert_eq!(
                signal_number(&name),
                Some(n),
                "signal {n} is called {name:?} and did not map back"
            );
        }

        // The floor is what makes this a claim. The `else { continue }` above is there
        // because a platform need not name every number, but it also means a `signal_name`
        // that answered for nothing at all would sail through the loop having proved
        // nothing — stubbing it to know only SIGKILL left this test green.
        assert!(
            checked >= 28,
            "libc named only {checked} of signals 1..=31; the round trip proved almost nothing"
        );

        // And by name, the three the rest of this crate and `cide-app`'s ladder reason
        // about: a table that lost exactly these would still clear the floor above.
        for wanted in [libc::SIGHUP, libc::SIGKILL, libc::SIGTERM] {
            let name = signal_name(wanted).unwrap_or_else(|| panic!("libc will not name {wanted}"));
            assert_eq!(
                signal_number(&name),
                Some(wanted),
                "{name:?} did not map back"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn an_unmappable_signal_name_falls_back_rather_than_inventing_a_number() {
        // Two shapes of "we cannot tell": portable-pty's own `Signal N` spelling, which
        // carries the number in the text, and a name from nowhere, which carries nothing.
        assert_eq!(signal_number("Signal 9"), Some(9));
        assert_eq!(signal_number("not a signal anybody named"), None);

        // A number outside the range `128 + n` can be built from is refused rather than
        // believed. `128 + 2147483647` overflows `i32`, which panics a debug build — a
        // process-wide crash to report an exit code, from a string that arrived as data.
        assert_eq!(signal_number("Signal 2147483647"), None);
        assert_eq!(signal_number("Signal 0"), None);
        assert_eq!(
            classify(Ok(portable_pty::ExitStatus::with_signal(
                "Signal 2147483647"
            )))
            .code,
            1,
            "an out-of-range number must report the flat status, not overflow `128 + n`"
        );

        let exit = classify(Ok(portable_pty::ExitStatus::with_signal(
            "not a signal anybody named",
        )));
        assert_eq!(
            exit.code, 1,
            "an unmappable name must report the flat status, not `128 + garbage`"
        );
        assert_eq!(exit.signal.as_deref(), Some("not a signal anybody named"));
    }

    #[test]
    fn a_wait_that_failed_reports_unknown_rather_than_success() {
        // The one path that has no answer. Reporting 0 here would be a fabricated success
        // — the single value that could later be mistaken for a real one.
        let exit = classify(Err(std::io::Error::from_raw_os_error(10)));
        assert_eq!(exit.code, UNKNOWN_EXIT_CODE);
        assert_eq!(exit.signal, None);
        assert!(!exit.is_success());
    }

    #[test]
    fn a_watcher_registered_before_the_child_dies_is_called_once() {
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            // Long enough that registration is comfortably first, short enough that the
            // test does not sit on it.
            .arg("sleep 0.3; exit 5");
        let session = PtySession::spawn(spec).expect("spawn sh");

        let (tx, rx) = mpsc::channel::<Exit>();
        session.on_exit(move |exit| {
            let _ = tx.send(exit);
        });

        let exit = rx
            .recv_timeout(DEADLINE)
            .expect("the watcher was never called");
        assert_eq!(exit.code, 5);
        assert!(
            rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "the watcher fired more than once"
        );
    }

    #[test]
    fn a_watcher_registered_after_the_child_dies_still_fires() {
        // A one-shot child can be gone before `spawn` has returned. A watcher that silently
        // missed its event is how a pane ends up showing a live cursor on a dead process —
        // which is why this is answered from the stored status rather than parked for an
        // event that already happened.
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("exit 3");
        let session = PtySession::spawn(spec).expect("spawn sh");
        let reaped = await_exit(&session);
        assert_eq!(reaped.code, 3);

        let (tx, rx) = mpsc::channel::<Exit>();
        session.on_exit(move |exit| {
            let _ = tx.send(exit);
        });
        let exit = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("a late watcher was dropped on the floor");
        assert_eq!(exit.code, 3);
    }

    #[test]
    fn every_watcher_gets_the_answer() {
        // The app registers one, but the mirror/detach story means "one pane per session"
        // has never been a safe assumption here.
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("sleep 0.2; exit 4");
        let session = PtySession::spawn(spec).expect("spawn sh");

        let (tx, rx) = mpsc::channel::<Exit>();
        for _ in 0..3 {
            let tx = tx.clone();
            session.on_exit(move |exit| {
                let _ = tx.send(exit);
            });
        }
        drop(tx);

        let mut seen = Vec::new();
        while let Ok(exit) = rx.recv_timeout(DEADLINE) {
            seen.push(exit.code);
        }
        assert_eq!(seen, vec![4, 4, 4], "a parked watcher was skipped");
    }

    #[test]
    fn settling_hands_the_watchers_back_rather_than_running_them() {
        // `has_exited()` is what `cide-app`'s shutdown ladder polls, and watchers are
        // arbitrary caller code — the one that ships emits a Tauri event. So the reaper has
        // one more thing to do, `exited.store(true)`, before any of them gets the thread,
        // and `settle` returning them instead of calling them is what leaves that order the
        // reaper's to choose. This is the unit that can be checked: end to end the coalescer
        // sets the same flag on EOF, which Linux delivers as soon as the pty's session
        // leader dies, so an integration test of the order would pass either way.
        //
        // The status has to be published before anyone is called, too — a watcher that asks
        // `exit_status()` must not be told `None` about the very exit it is being handed.
        let ran = Arc::new(AtomicBool::new(false));
        let slot = {
            let ran = Arc::clone(&ran);
            let callback: ExitCallback = Box::new(move |_| ran.store(true, Ordering::Release));
            Mutex::new(ExitSlot::Running(vec![callback]))
        };

        let exit = Exit {
            code: 7,
            signal: None,
        };
        let waiting = settle(&slot, &exit);

        assert_eq!(
            waiting.len(),
            1,
            "settle swallowed the parked watcher instead of handing it back"
        );
        assert!(
            !ran.load(Ordering::Acquire),
            "settle called the watcher itself; the reaper can no longer order the two"
        );
        assert!(
            matches!(&*slot.lock(), ExitSlot::Reaped(settled) if settled == &exit),
            "the status was not published before the watchers were handed over"
        );

        for callback in waiting {
            callback(exit.clone());
        }
        assert!(ran.load(Ordering::Acquire));
    }

    #[test]
    fn exit_status_is_none_while_the_child_runs() {
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("sleep 30");
        let session = PtySession::spawn(spec).expect("spawn sh");
        assert_eq!(session.exit_status(), None);
        assert!(!session.has_exited());
        session.kill();
    }

    #[test]
    fn small_writes_are_coalesced_into_few_frames() {
        // 2000 one-byte writes with no delay must not become 2000 IPC frames; if they do,
        // every frame is under Tauri's 1024-byte raw threshold and takes the eval path.
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("for i in $(seq 1 2000); do printf x; done");
        let session = PtySession::spawn(spec).expect("spawn sh");

        let (tx, rx) = mpsc::channel::<usize>();
        let tx = Mutex::new(tx);
        let sink: Arc<dyn Sink> = Arc::new(move |b: &[u8]| tx.lock().send(b.len()).is_ok());
        session.attach(sink);

        let deadline = Instant::now() + DEADLINE;
        let mut frames = 0usize;
        let mut total = 0usize;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(remaining.min(Duration::from_millis(400))) {
                Ok(n) => {
                    frames += 1;
                    total += n;
                }
                Err(_) if total > 0 => break,
                Err(_) => break,
            }
        }
        assert!(
            total >= 2000,
            "only saw {total} bytes across {frames} frames"
        );
        assert!(
            frames < 200,
            "{frames} frames for {total} bytes — coalescing is not working"
        );
    }

    // --- credit flow control ---------------------------------------------------------
    //
    // Driven against `broadcast` directly rather than through a real child. The state
    // machine is pure — bytes in, frames out — and forcing megabytes through a PTY to
    // observe it made the whole suite slow, load-sensitive and flaky without testing one
    // thing more. The real-PTY tests above still cover the path that reaches these.

    /// Every frame a recording sink was handed, in order.
    type FrameLog = Arc<Mutex<Vec<Vec<u8>>>>;

    /// A sink that records every frame and never acks, unless the test acks for it.
    fn recording() -> (Arc<dyn Sink>, FrameLog) {
        let log: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
        let l = Arc::clone(&log);
        let sink: Arc<dyn Sink> = Arc::new(move |b: &[u8]| {
            l.lock().push(b.to_vec());
            true
        });
        (sink, log)
    }

    struct Harness {
        sinks: Arc<Mutex<Vec<Registered>>>,
        vt: Arc<Mutex<vt100::Parser>>,
        policy: CreditPolicy,
    }

    impl Harness {
        fn new(policy: CreditPolicy) -> Self {
            Self {
                sinks: Arc::new(Mutex::new(Vec::new())),
                vt: Arc::new(Mutex::new(vt100::Parser::new(24, 80, 100))),
                policy,
            }
        }

        fn attach(&self, sink: Arc<dyn Sink>) -> SinkId {
            let id = SinkId(self.sinks.lock().len() as u64 + 1);
            self.sinks.lock().push(Registered {
                id,
                sink,
                outstanding: AtomicUsize::new(0),
                choked: AtomicBool::new(false),
                missed: AtomicBool::new(false),
                last_ack: Mutex::new(Instant::now()),
            });
            id
        }

        /// One frame of output, fed to the mirror exactly as the coalescer would.
        fn feed(&self, bytes: &[u8]) {
            self.vt.lock().process(bytes);
            broadcast(
                &self.sinks,
                &self.vt,
                &self.policy,
                Frame::Bytes(bytes.to_vec()),
            );
        }

        fn tick(&self) {
            broadcast(&self.sinks, &self.vt, &self.policy, Frame::Tick);
        }

        fn with<T>(&self, id: SinkId, f: impl Fn(&Registered) -> T) -> T {
            let g = self.sinks.lock();
            f(g.iter().find(|r| r.id == id).expect("sink present"))
        }

        fn outstanding(&self, id: SinkId) -> usize {
            self.with(id, |r| r.outstanding.load(Ordering::Acquire))
        }

        fn choked(&self, id: SinkId) -> bool {
            self.with(id, |r| r.choked.load(Ordering::Acquire))
        }

        fn ack(&self, id: SinkId, n: usize) {
            self.with(id, |r| {
                let _ = r
                    .outstanding
                    .fetch_update(Ordering::Release, Ordering::Acquire, |c| {
                        Some(c.saturating_sub(n))
                    });
                *r.last_ack.lock() = Instant::now();
            });
        }
    }

    fn policy(high: usize, low: usize, watchdog: Duration) -> CreditPolicy {
        CreditPolicy {
            high,
            low,
            watchdog,
        }
    }

    #[test]
    fn a_sink_that_never_acks_stops_receiving_raw_output() {
        let h = Harness::new(policy(1000, 200, Duration::from_secs(3600)));
        let (sink, log) = recording();
        let id = h.attach(sink);

        for _ in 0..20 {
            h.feed(&vec![b'x'; 400]);
        }

        assert!(h.choked(id), "a sink that never acked was never choked");
        let delivered: usize = log.lock().iter().map(|f| f.len()).sum();
        assert!(
            delivered < 8000,
            "{delivered} bytes reached a sink that never acked — credit is not holding"
        );
    }

    #[test]
    fn a_keeping_up_sink_is_never_choked() {
        // The common case must not pay for the pathological one.
        let h = Harness::new(policy(1000, 200, Duration::from_secs(3600)));
        let (sink, log) = recording();
        let id = h.attach(sink);

        for _ in 0..50 {
            h.feed(&vec![b'x'; 400]);
            h.ack(id, 400);
        }

        assert!(!h.choked(id));
        assert_eq!(h.outstanding(id), 0);
        assert_eq!(log.lock().len(), 50, "a healthy sink missed frames");
    }

    #[test]
    fn a_choked_sink_recovers_on_a_tick_after_acking() {
        // The bug the first version of this shipped with: unchoking only happened when the
        // next frame of real output arrived, so a sink that choked during a burst and then
        // went quiet — exactly what a finished `cat` looks like — sat on stale content
        // waiting for a frame that was never coming.
        let h = Harness::new(policy(1000, 200, Duration::from_secs(3600)));
        let (sink, log) = recording();
        let id = h.attach(sink);

        for _ in 0..20 {
            h.feed(&vec![b'x'; 400]);
        }
        assert!(h.choked(id), "never choked");
        let frames_at_choke = log.lock().len();

        // Output stops. The consumer catches up and acks everything it owes.
        h.ack(id, h.outstanding(id));
        h.tick();

        assert!(!h.choked(id), "a fully-acked sink stayed choked");
        assert_eq!(
            log.lock().len(),
            frames_at_choke + 1,
            "recovery did not deliver the catch-up frame"
        );
    }

    #[test]
    fn a_wedged_sink_is_forgiven_by_the_watchdog() {
        // The deadlock this mechanism exists to prevent: a webview stops acking because its
        // GPU context was lost. Without the watchdog it is pinned above `high` for ever and
        // simply looks frozen, with nothing anywhere to explain why.
        let h = Harness::new(policy(1000, 200, Duration::from_millis(30)));
        let (sink, log) = recording();
        let id = h.attach(sink);

        for _ in 0..20 {
            h.feed(&vec![b'x'; 400]);
        }
        assert!(h.choked(id), "never choked");
        let frames_at_choke = log.lock().len();

        // It never acks. Time passes.
        thread::sleep(Duration::from_millis(60));
        h.tick();

        assert!(!h.choked(id), "the watchdog never released a wedged sink");
        assert!(
            log.lock().len() > frames_at_choke,
            "a wedged sink stayed silent for ever"
        );
    }

    #[test]
    fn a_resumed_sink_gets_the_screen_not_a_mid_stream_fragment() {
        // Splicing a sink back into the raw stream resumes it mid-escape-sequence. It gets
        // the screen mirror instead, which is self-consistent and — crucially — contains
        // the content written while it was choked.
        let h = Harness::new(policy(500, 100, Duration::from_secs(3600)));
        let (sink, log) = recording();
        let id = h.attach(sink);

        h.feed(b"BEFORE-CHOKE\r\n");
        h.ack(id, 0); // no credit returned; it is now behind
        for _ in 0..10 {
            h.feed(&vec![b'x'; 400]);
        }
        assert!(h.choked(id), "never choked");

        // Content the sink cannot have seen as raw bytes, because it is choked.
        h.feed(b"\r\nWHILE-CHOKED\r\n");
        let raw_seen = log.lock().concat();
        assert!(
            !String::from_utf8_lossy(&raw_seen).contains("WHILE-CHOKED"),
            "a choked sink was still being sent raw output"
        );

        h.ack(id, h.outstanding(id));
        h.tick();

        let catchup = log.lock().last().cloned().expect("a catch-up frame");
        let text = String::from_utf8_lossy(&catchup);
        assert!(
            text.contains("WHILE-CHOKED"),
            "the catch-up frame did not carry what the sink missed"
        );
    }

    #[test]
    fn one_wedged_sink_does_not_starve_a_healthy_one() {
        // The reason credit is per-sink. A mirrored pane that wedges must not freeze the
        // pane the user is actually looking at.
        let h = Harness::new(policy(1000, 200, Duration::from_secs(3600)));
        let (wedged, wedged_log) = recording();
        let (healthy, healthy_log) = recording();
        let w = h.attach(wedged);
        let ok = h.attach(healthy);

        for _ in 0..20 {
            h.feed(&vec![b'x'; 400]);
            h.ack(ok, 400);
        }

        assert!(h.choked(w), "the wedged sink was not choked");
        assert!(!h.choked(ok), "the healthy sink was choked with it");
        assert_eq!(healthy_log.lock().len(), 20, "the healthy sink lost frames");
        assert!(wedged_log.lock().len() < 20);
    }

    #[test]
    fn a_tick_delivers_nothing_to_a_sink_that_is_up_to_date() {
        let h = Harness::new(CreditPolicy::default());
        let (sink, log) = recording();
        h.attach(sink);

        h.feed(b"hello");
        for _ in 0..10 {
            h.tick();
        }
        assert_eq!(
            log.lock().len(),
            1,
            "ticks are leaking empty frames to sinks"
        );
    }

    #[test]
    fn an_over_ack_cannot_wrap_the_counter() {
        // An ack for a frame delivered before a watchdog reset arrives against a counter
        // that has already been zeroed. Wrapping here would choke the sink permanently —
        // the exact deadlock this mechanism exists to prevent, reintroduced by arithmetic.
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("exit 0");
        let session = PtySession::spawn(spec).expect("spawn sh");
        let sink: Arc<dyn Sink> = Arc::new(|_: &[u8]| true);
        let id = session.attach(sink);

        session.ack(id, usize::MAX);
        assert_eq!(session.outstanding(id), 0, "credit wrapped past zero");
        assert!(!session.is_choked(id));
    }

    #[test]
    fn detaching_takes_the_debt_with_it() {
        let h = Harness::new(policy(1000, 200, Duration::from_secs(3600)));
        let (sink, _log) = recording();
        let id = h.attach(sink);
        for _ in 0..20 {
            h.feed(&vec![b'x'; 400]);
        }
        assert!(h.choked(id));

        h.sinks.lock().retain(|r| r.id != id);

        // A fresh attachment starts clear rather than inheriting a debt it never incurred.
        let (sink2, _l2) = recording();
        let id2 = h.attach(sink2);
        assert_eq!(h.outstanding(id2), 0);
        assert!(!h.choked(id2));
    }

    /// M2's load check. `--ignored`, because it moves a gigabyte and is a deliberate act.
    ///
    /// Two failure modes it exists to catch, neither of which shows up at small sizes:
    /// a consumer that cannot keep up deadlocking the pipeline, and the tail of a long
    /// stream being lost or reordered once credit starts skipping frames.
    #[test]
    #[ignore = "moves 1 GiB; run with --ignored"]
    fn a_gigabyte_streams_without_deadlock_and_keeps_the_tail() {
        let script = "head -c 1073741824 /dev/zero | tr '\\0' 'x'; printf '\\nTAIL-MARKER\\n'";
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg(script);
        let session = PtySession::spawn(spec).expect("spawn sh");

        // A sink that acks lazily — the realistic case, not the ideal one.
        let delivered = Arc::new(AtomicUsize::new(0));
        let d = Arc::clone(&delivered);
        let sink: Arc<dyn Sink> = Arc::new(move |b: &[u8]| {
            d.fetch_add(b.len(), Ordering::Relaxed);
            true
        });
        let id = session.attach(sink);

        let started = Instant::now();
        let deadline = started + Duration::from_secs(120);
        while !session.has_exited() && Instant::now() < deadline {
            // Ack at a fraction of the arrival rate, so the sink spends the run choked.
            session.ack(id, 4096);
            thread::sleep(Duration::from_millis(1));
        }

        assert!(
            session.has_exited(),
            "1 GiB did not drain in 120s — the pipeline deadlocked against a slow consumer"
        );

        // The tail is the point: a choked sink must still end up showing the end of the
        // stream, because catch-up frames come from the mirror rather than the raw bytes.
        let screen = String::from_utf8_lossy(&session.screen_state()).into_owned();
        assert!(
            screen.contains("TAIL-MARKER"),
            "the final output never reached the screen mirror"
        );

        eprintln!(
            "1 GiB in {:?}; {} bytes delivered to a deliberately slow sink",
            started.elapsed(),
            delivered.load(Ordering::Relaxed)
        );
    }

    #[test]
    fn a_preload_is_in_the_mirror_before_the_child_says_anything() {
        // The restored-shell path: the previous run's screen is handed to `spawn` and has to
        // be in the snapshot the first attaching pane receives, ahead of anything the new
        // child prints. `sleep` prints nothing at all, so what comes back is the preload.
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("sleep 5")
            .preload("PREVIOUS-RUN".as_bytes().to_vec());
        let session = PtySession::spawn(spec).expect("spawn sh");

        let (_, screen) = session.attach_with_snapshot(Arc::new(|_: &[u8]| true));
        let text = String::from_utf8_lossy(&screen).into_owned();
        assert!(
            text.contains("PREVIOUS-RUN"),
            "the preload never reached the screen mirror: {text:?}"
        );
        session.kill();
    }

    #[test]
    fn a_preload_is_never_typed_at_the_child() {
        // The failure this pins is the one that would be catastrophic rather than merely
        // wrong: preloaded bytes going down the *pty* would be executed by the shell. `cat`
        // echoes its stdin, so if the preload had reached the child it would come back out
        // and land on the screen twice.
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("cat")
            .preload("ZZMARKERZZ\r\n".as_bytes().to_vec());
        let session = PtySession::spawn(spec).expect("spawn sh");
        thread::sleep(Duration::from_millis(200));

        let text = String::from_utf8_lossy(&session.screen_state()).into_owned();
        assert_eq!(
            text.matches("ZZMARKERZZ").count(),
            1,
            "the preload was written to the child, not only to the mirror: {text:?}"
        );
        session.kill();
    }

    /// A preload is not only pixels: its DEC private modes stick to the new session.
    ///
    /// This is the hazard `cide_app::lifecycle::NEUTRAL_MODES` exists to answer, pinned here
    /// because here is where it is true. `screen_state()` ends with the screen's *input* modes,
    /// so a screen saved while a TUI held mouse tracking and a hidden cursor carries both — and
    /// a preload feeds them straight into the mirror of a child that never asked for either and
    /// will never turn them off. Every pane that attaches then gets them, because the snapshot
    /// is generated from this same mirror.
    ///
    /// If this ever stops being true the neutralising tail is dead weight and can go; until
    /// then, deleting it puts a restored shell in mouse-reporting mode with no cursor.
    ///
    /// One `#[test]`, deliberately, and one child. The tests around here are timing-sensitive —
    /// `small_writes_are_coalesced_into_few_frames` measures a 4 ms coalescing window — and
    /// every extra test in this module is another pty, another four threads and another process
    /// competing with it under `cargo test`'s parallelism. A second spawn saying the mirrored
    /// half of this was enough to make that test flake once in three runs on this machine.
    /// That the DECRSTs then clear these modes is `vt100`'s own contract and is asserted
    /// bytewise, without a process, in `cide_app::lifecycle`'s
    /// `a_replay_turns_off_the_modes_the_old_screen_turned_on`.
    #[test]
    fn a_preload_carries_the_old_childs_input_modes_into_the_mirror() {
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("sleep 5")
            .preload("\x1b[?25l\x1b[?1002h\x1b[?1006h".as_bytes().to_vec());
        let session = PtySession::spawn(spec).expect("spawn sh");

        let text = String::from_utf8_lossy(&session.screen_state()).into_owned();
        assert!(
            text.contains("\u{1b}[?25l"),
            "a hidden cursor in the preload survives into the new session: {text:?}"
        );
        assert!(
            text.contains("\u{1b}[?1002h"),
            "mouse tracking in the preload survives into the new session: {text:?}"
        );
        session.kill();
    }

    #[test]
    fn resize_updates_the_screen_mirror() {
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("sleep 5")
            .geometry(Geometry::new(80, 24, 8, 17));
        let session = PtySession::spawn(spec).expect("spawn sh");
        session
            .resize(Geometry::new(120, 40, 8, 17))
            .expect("resize");
        assert_eq!(session.geometry().cols, 120);
        assert_eq!(session.geometry().rows, 40);
        session.kill();
    }
}
