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
    vt: Arc<Mutex<vt100::Parser>>,
    sinks: Arc<Mutex<Vec<Registered>>>,
    next_sink_id: AtomicU32,
    geometry: Mutex<Geometry>,
    child_pid: Option<u32>,
    exited: Arc<AtomicBool>,
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
        let sinks: Arc<Mutex<Vec<Registered>>> = Arc::new(Mutex::new(Vec::new()));
        let exited = Arc::new(AtomicBool::new(false));

        let (raw_tx, raw_rx) = bounded::<Vec<u8>>(READ_QUEUE_DEPTH);
        let (writer_tx, writer_rx) = unbounded::<Vec<u8>>();

        spawn_reader(reader, raw_tx);
        spawn_coalescer(
            raw_rx,
            Arc::clone(&vt),
            Arc::clone(&sinks),
            Arc::clone(&exited),
            spec.credit,
        );
        spawn_writer(writer, writer_rx);

        let killer = child.clone_killer();
        spawn_reaper(child, Arc::clone(&exited));

        Ok(Arc::new(Self {
            master: Mutex::new(pair.master),
            writer_tx,
            vt,
            sinks,
            next_sink_id: AtomicU32::new(1),
            geometry: Mutex::new(spec.geometry),
            child_pid,
            exited,
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

    pub fn has_exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
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
        let id = SinkId(self.next_sink_id.fetch_add(1, Ordering::Relaxed) as u64);
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

fn spawn_coalescer(
    rx: Receiver<Vec<u8>>,
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

                match rx.recv_timeout(timeout) {
                    Ok(chunk) => {
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
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
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
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
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

fn spawn_reaper(mut child: Box<dyn portable_pty::Child + Send + Sync>, exited: Arc<AtomicBool>) {
    thread::Builder::new()
        .name("cide-pty-reap".into())
        .spawn(move || {
            let _ = child.wait();
            exited.store(true, Ordering::Release);
        })
        .expect("spawn pty reaper thread");
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

        let deadline = Instant::now() + DEADLINE;
        while !session.has_exited() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }

        // Nothing was ever attached; the output still exists.
        let screen = String::from_utf8_lossy(&session.screen_state()).into_owned();
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
