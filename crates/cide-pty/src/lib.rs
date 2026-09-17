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

pub mod jobs;
pub use jobs::{JobEvent, JobWatch};

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

/// Rewrites a session's output, one line at a time, before anything downstream sees it.
///
/// # What this is for, and the shape of the stream it exists for
///
/// A headless harness child (`opencode run --format json`) emits machine events — ndjson, one
/// event per line — and that stream is simultaneously the *channel* something in the app parses
/// for run state and the *picture* a pane shows when a person opens the run. Raw, the picture is
/// unreadable. This hook is the reconciliation: it runs at the **top of the coalescer's output
/// arm**, so the vt100 mirror, every sink, the frame splitter and the choked-sink catch-up all
/// operate on one rendered stream and cannot disagree — a catch-up frame is rendered from the
/// mirror, and a mirror holding different text than the sinks is precisely the corruption the
/// coalescer exists to prevent. The caller that needs the *raw* line reads it inside this hook,
/// before returning the rendering; there is deliberately no second, raw sink class.
///
/// # The contract
///
/// Called once per complete line, on the coalescer thread, with the terminator (and a trailing
/// `\r`) stripped. `Some(text)` replaces the line — it may span several lines, and the plumbing
/// converts its `\n`s to the `\r\n` a terminal needs, since these bytes bypass the pty's own
/// output post-processing. `None` drops the line from the display entirely. A line that is not
/// the hook's format should be returned verbatim, not dropped: this stream is also where a
/// child's own error text arrives.
///
/// It runs on the thread every byte of every session flows through, so the same law as
/// [`Sink::deliver`] applies: never block, never call back into this session. Unlike a sink it
/// runs *outside* the sink-list lock and owes no acknowledgement — which is much of why the
/// app's stream observer moved into it.
#[derive(Clone)]
pub struct LineRender {
    render: LineRenderFn,
    /// Whether a chunk's unterminated tail may be *held* for the next chunk to complete.
    ///
    /// `None` holds every tail, which is right for a stream whose every line ends in `\n` —
    /// a `--format json` harness child — and catastrophic for an interactive shell, whose
    /// most important output has no terminator at all: a prompt (`user@host:~$ `), a
    /// fullscreen TUI's screen, a `\r`-only progress bar. Held, a prompt is invisible until
    /// the user presses Enter, which reads as a hung pane.
    ///
    /// `Some(p)` asks the hook, once per chunk, whether the remainder could still *become* a
    /// line it would rewrite. A `false` sends the tail out raw immediately and resyncs, so a
    /// prompt costs nothing: a prompt does not look like the start of a JSON object.
    partial: Option<PartialFn>,
}

/// The closure inside [`LineRender`], named so the field stays legible to clippy and callers
/// alike.
pub type LineRenderFn = Arc<dyn Fn(&str) -> Rendered + Send + Sync>;

/// [`LineRender::holding_only`]'s predicate: *could this partial line still become one you
/// would rewrite?*
pub type PartialFn = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// What [`LineRender`] decided about one line.
///
/// Three states rather than an `Option`, and [`Rendered::Keep`] is why. "Pass this through"
/// used to be spelled `Some(line.to_string())`, which round-trips the *text* and loses the
/// original bytes: the line is re-terminated `\r\n`, and a program in raw mode (`-opost`, what
/// every fullscreen TUI sets) emits a bare `\n` meaning *down one row, same column*. Rewriting
/// that to CRLF moves the cursor to column 0 and shifts the picture, silently and only for
/// people running a TUI. `Keep` copies the original slice and the terminator it actually had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rendered {
    /// Not this hook's format — emit the original bytes, terminator included, untouched.
    Keep,
    /// Drop the line from the display entirely.
    Drop,
    /// Replace it. The text may span several lines; its `\n`s become `\r\n`, since these
    /// bytes bypass the pty's own output post-processing.
    Replace(String),
}

impl Rendered {
    /// The replacement text, for a caller that cares only about what a line became — a test
    /// asserting on a rendering, rather than the splitter, which has to tell all three apart.
    pub fn text(&self) -> Option<&str> {
        match self {
            Rendered::Replace(text) => Some(text),
            _ => None,
        }
    }
}

impl LineRender {
    /// A renderer for a stream whose every line is newline-terminated. Holds any tail.
    pub fn new(render: LineRenderFn) -> Self {
        Self {
            render,
            partial: None,
        }
    }

    /// A renderer safe to install on an *interactive* stream: hold a tail only while
    /// `partial` says it could still complete into something this hook rewrites. See the
    /// field's own comment for what holding one unconditionally does to a shell prompt.
    pub fn holding_only(mut self, partial: PartialFn) -> Self {
        self.partial = Some(partial);
        self
    }

    fn line(&self, line: &str) -> Rendered {
        (self.render)(line)
    }

    /// Whether the held remainder is worth keeping, and `true` when no predicate was given.
    fn holds(&self, tail: &[u8]) -> bool {
        match self.partial.as_ref() {
            Some(partial) => partial(&String::from_utf8_lossy(tail)),
            None => true,
        }
    }

    /// Whether a tail that has gone quiet should be flushed. Only an interactive stream has
    /// the problem — see [`RenderState`] — so only a renderer with a predicate answers yes.
    fn flushes_when_idle(&self) -> bool {
        self.partial.is_some()
    }
}

impl std::fmt::Debug for LineRender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The pointee is an opaque closure; naming the type is everything there is to print.
        f.write_str("LineRender(..)")
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
    ///
    /// `screen_state()` and not [`PtySession::reattach_state`], deliberately, and the
    /// difference is load-bearing here: the new child is a fresh shell on the *normal*
    /// buffer, so a preload that dragged the mirror onto the alternate one — which is what a
    /// user who quit cide with `vim` on screen would produce — would leave the replayed
    /// picture in a grid nobody is showing and send every byte the new child writes there
    /// too. `screen_state()` is the buffer-agnostic picture, which is the right shape for a
    /// replay.
    pub preload: Vec<u8>,
    /// Keep the pty and the mirror at the spawn geometry for the child's whole life — every
    /// later [`PtySession::resize`] is a no-op. (M42)
    ///
    /// For a child whose output is **lines, not a screen**: an agent run on a harness that
    /// prints machine events (`opencode run --format json`) neither reads the terminal size nor
    /// paints into it, so the only thing a resize does to it is *damage the mirror*. `vt100`'s
    /// `set_size` does not reflow: it truncates every row to the new width and clears every
    /// wrap flag, so a mirror made wide enough to hold a rendered line whole is cut to the first
    /// pane that attaches narrower, and one made narrow (the 80-column default a headless run
    /// used to get) hands every later pane its history as 80-column fragments with hard breaks
    /// — the "text is not on full width" report. Fixed and wide, the mirror wraps almost
    /// nothing, what it does wrap keeps its flag for the replay to join, and the pane's own
    /// terminal soft-wraps each logical line at whatever width it actually has.
    pub fixed_size: bool,
    /// Announce foreground jobs in this pane that run at least this long.
    ///
    /// `None` — the default, and what every Claude pane uses — watches nothing and costs
    /// nothing. A Claude session learns its state from hooks (`cide_claude::next_state`),
    /// which are exact and know about permission prompts; watching its process group as well
    /// would report every tool call the CLI forks as a job of its own and fight the hooks for
    /// the same `SessionState`.
    ///
    /// A shell has no hooks, so this is the only thing that can tell a pane the `make` it was
    /// running has finished. See [`crate::jobs`] for what is watched and why there is a
    /// threshold at all.
    pub watch_jobs: Option<Duration>,
    /// Rewrite this session's output line-by-line before the mirror or any sink sees it.
    ///
    /// `None` — every pane, every claude child — is the zero-cost path: bytes flow untouched.
    /// See [`LineRender`] for the contract and the reason there is no raw sink class beside it.
    pub render: Option<LineRender>,
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
            fixed_size: false,
            watch_jobs: None,
            render: None,
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

    /// Fold a list of environment *changes* — `(name, Some(value))` sets, `(name, None)`
    /// removes — into this spec.
    ///
    /// The shape is `cide_core::child_env::EnvChange`, spelled structurally because
    /// `cide-pty` does not depend on `cide-core` and must not: the dependency runs the other
    /// way, and the domain crate is what composes these lists.
    ///
    /// That split is the whole reason this method exists. `cide-core` owns the *rules* about
    /// what a child's environment must be — the bundle scrub, the `PATH` append, the
    /// `CLAUDE_CODE_*` switches, the terminal constants — and hands back one ordered list,
    /// because it cannot see [`SpawnSpec`]. Folding that list is the half only this crate can
    /// do. Keeping the fold here means every spawn site that takes a composed list folds it
    /// the same way, instead of each one reimplementing the two-arm `match` and one of them
    /// eventually dropping the removal arm — which would leave a variable the rule said must
    /// go sitting in the child, with nothing anywhere to say so.
    ///
    /// Order is preserved into [`SpawnSpec::env`] and [`SpawnSpec::env_remove`], which are
    /// separate lists: removals are applied to the inherited environment first, then the sets,
    /// so a set and a removal of the same name do not race on their position in this list.
    pub fn apply(self, changes: impl IntoIterator<Item = (String, Option<String>)>) -> Self {
        changes
            .into_iter()
            .fold(self, |spec, (name, value)| match value {
                Some(value) => spec.env(name, value),
                None => spec.env_remove(name),
            })
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

    /// See [`SpawnSpec::fixed_size`].
    pub fn fixed_size(mut self) -> Self {
        self.fixed_size = true;
        self
    }

    /// Render this session's output for display. See [`LineRender`].
    pub fn render(mut self, render: LineRender) -> Self {
        self.render = Some(render);
        self
    }

    /// Watch this pane's foreground process group. See [`SpawnSpec::watch_jobs`].
    pub fn watch_jobs(mut self, announce_after: Duration) -> Self {
        self.watch_jobs = Some(announce_after);
        self
    }
}

/// A consumer of session output.
///
/// Implemented over a Tauri `Channel` in the app crate, and over a plain channel in tests
/// and in `cide-headless`. Returning `false` means the sink is gone and should be dropped.
///
/// # `deliver` runs with the sink list locked, so it must never re-enter this session
///
/// `PtySession::broadcast` (private, below) walks the registered sinks under their own mutex
/// and calls
/// `deliver` from inside that walk. `parking_lot::Mutex` is **not reentrant**, so calling
/// anything on the same [`PtySession`] from within `deliver` parks the caller for ever — and
/// the caller is the coalescer thread, which is the single thread every byte of every session
/// in the process flows through. The symptom is that **every terminal in the application stops
/// painting**, with no panic, no log line and no error anywhere: the one failure mode in this
/// crate that reports nothing at all.
///
/// [`PtySession::ack`] is the one that gets reached for, because a sink that consumes bytes is
/// exactly the thing that wants to return credit — M18's agent stream sink did precisely this
/// and wedged the coalescer. Acknowledge from **another thread** instead. Not acknowledging at
/// all is not the escape either: the sink chokes at its credit limit and starts receiving
/// rendered screens in place of the byte stream, which silently corrupts any consumer that is
/// parsing rather than painting.
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
        /// Whether the reply should carry the retained scrollback in front of the screen. See
        /// [`PtySession::attach_with_snapshot`].
        history: bool,
        /// The screen as of the cut point. The caller sends this to the sink itself.
        reply: Sender<Vec<u8>>,
    },
    /// Retune [`JobWatch::set_announce_after`] on the coalescer's probe, if it has one.
    ///
    /// A control message rather than a shared atomic so the watch keeps exactly one writer —
    /// see [`PtySession::set_job_announce_after`].
    JobThreshold(Duration),
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

/// Where a session's bytes come from and go to. (M42)
///
/// # Why this exists
///
/// Everything downstream of the reader thread in this file — the bounded channel, the
/// coalescer, [`FLUSH_BYTES`]/[`FLUSH_INTERVAL`], the vt100 mirror, the sink list,
/// [`CreditPolicy`] and the credit watchdog — is a statement about *a stream of terminal
/// bytes*, not about a pty. M42 needed a second kind of stream: a `docker exec` hijacked
/// connection, and a `docker logs -f` follow. Reimplementing that machinery for them would
/// have meant a second terminal stack with a second answer to backpressure, reattach and
/// gapless detach, which is the most expensive and most load-bearing code in this repository.
///
/// So the pty becomes one implementation of this and the rest of the file stops knowing.
/// [`PtySession::spawn`] is unchanged in behaviour and is now a wrapper over
/// [`PtySession::connect`].
///
/// # Two rules, both silent when broken
///
/// **[`Self::foreground_pgid`] returning `None` must mean the jobs watcher observes nothing**,
/// not that it polls something else. `cide_app::lifecycle::watch_jobs` announces `Busy` and
/// `AwaitingInput` from `tcgetpgrp` against the child's own pid; a transport with no process
/// group has no such comparison to make, and [`JobWatch::observe`] already treats `None` as *no
/// information* rather than as a prompt. The default implementation is therefore the correct
/// one for everything that is not a local pty, and overriding it is the exceptional act.
///
/// **[`Self::kill`] must be idempotent and must not block.** It is called from the shutdown
/// ladder, possibly several times, possibly on a transport whose far end is already gone.
pub trait Transport: Send + Sync + 'static {
    /// Tell the far end the window changed size.
    fn resize(&self, geometry: Geometry) -> Result<(), PtyError>;

    /// Ask the far end to go away. Escalation is the caller's policy; see [`PtySession::kill`].
    fn kill(&self);

    /// Which process group owns the terminal, or `None` when that cannot be known.
    ///
    /// `None` is the honest answer for every transport that is not a local pty, and it is the
    /// default for that reason — see the trait's own note on why this is not a gap.
    fn foreground_pgid(&self) -> Option<i32> {
        None
    }
}

/// The local pty, which is what every session was before M42.
struct PtyTransport {
    /// Behind its own lock because [`Self::resize`] and `process_group_leader` both need it and
    /// they are called from different threads — the command worker and the coalescer.
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn portable_pty::ChildKiller + Send + Sync>>,
}

impl Transport for PtyTransport {
    fn resize(&self, geometry: Geometry) -> Result<(), PtyError> {
        self.master
            .lock()
            .resize(geometry.to_pty_size())
            .map_err(|e| PtyError::Resize(e.into()))
    }

    fn kill(&self) {
        let _ = self.killer.lock().kill();
    }

    /// # Why this is the one override
    ///
    /// `None` on a platform without the call is the same answer as `None` from a pty that has
    /// gone away, and [`JobWatch::observe`] treats it as no information rather than as a prompt
    /// — so a build for a target this is not implemented on simply never notices a job, which is
    /// exactly the pre-existing behaviour.
    #[cfg(unix)]
    fn foreground_pgid(&self) -> Option<i32> {
        self.master.lock().process_group_leader()
    }

    #[cfg(not(unix))]
    fn foreground_pgid(&self) -> Option<i32> {
        None
    }
}

/// A live PTY session.
///
/// Owned by the session registry in the app crate, never by a window, tab or pane —
/// closing any of those detaches a sink, it does not kill the child.
pub struct PtySession {
    /// Where the bytes come from — a local pty, or since M42 anything else that is a stream of
    /// terminal bytes. See [`Transport`].
    ///
    /// Shared with the coalescer, which asks it `tcgetpgrp` on a slow tick — see [`JobProbe`].
    /// An `Arc` rather than this struct's own field only for that: the coalescer thread is
    /// started inside [`PtySession::connect`], before this struct exists, so there is nothing
    /// else it could borrow from.
    transport: Arc<dyn Transport>,
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
    /// [`SpawnSpec::fixed_size`], carried so [`Self::resize`] can refuse.
    fixed_size: bool,
    child_pid: Option<u32>,
    exited: Arc<AtomicBool>,
    /// The reaper's answer. Separate from `exited` on purpose: `exited` is *also* set by the
    /// coalescer when the master reaches EOF, which can happen before — or, if a descendant
    /// is still holding the pty open, after — the child is actually reaped. `has_exited()`
    /// answers "is this pane dead" as early as possible, which is what the `— exited —`
    /// marker and the credit watchdog want; this answers "and how", which only the reaper
    /// knows.
    exit: Arc<Mutex<ExitSlot>>,
    credit: CreditPolicy,
    /// Who to tell when this pane's foreground goes to a job and comes back.
    ///
    /// Shared with the coalescer, which is what does the observing. Empty unless the spec
    /// asked for [`SpawnSpec::watch_jobs`], and empty for every Claude pane.
    job_listeners: JobListeners,
}

/// Registered [`PtySession::on_job`] callbacks.
///
/// A list rather than a slot for the same reason `sinks` is one, though today exactly one
/// consumer registers: `cide_app::lifecycle::watch_jobs`, once, at spawn.
type JobListeners = Arc<Mutex<Vec<Box<dyn Fn(JobEvent) + Send>>>>;

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

        let transport = Arc::new(PtyTransport {
            master: Mutex::new(pair.master),
            killer: Mutex::new(child.clone_killer()),
        });

        // The reaper's question, as a closure, so [`Self::connect`] never has to know what kind
        // of thing it is waiting for. `portable_pty::Child::wait` blocks, which is why it runs
        // on a thread of its own and not on any tick.
        let mut child = child;
        Ok(Self::connect(
            spec,
            reader,
            writer,
            transport,
            child_pid,
            Box::new(move || classify(child.wait())),
        ))
    }

    /// Build a session over any [`Transport`]. (M42)
    ///
    /// This is [`Self::spawn`]'s whole tail, and it is everything in this file that is *not*
    /// about a pty: the vt100 mirror, the bounded reader channel, the coalescer, the sink list,
    /// the writer and the reaper. A `docker exec` or a `docker logs -f` stream reaches it with a
    /// reader, a writer and a way to be waited on, and gets attach/detach, park, scrollback,
    /// credit and the reattach snapshot without another line.
    ///
    /// `wait` blocks and is run on the reaper thread. For a pty it is `child.wait()`; for a
    /// docker exec it is polling `GET /exec/{id}/json` for an `ExitCode`; for a log follow it is
    /// simply the stream ending.
    ///
    /// # The coalescer is not optional, whatever the transport
    ///
    /// A log follow has the pty's exact traffic profile — many small line-sized writes — and
    /// Tauri routes a raw `Channel` payload under 1024 bytes through `webview.eval` with the
    /// bytes spelled out as a JSON array of decimal numbers, on the GTK main loop. Every stream
    /// that reaches a sink must come through here for that reason; see this module's header.
    pub fn connect(
        spec: SpawnSpec,
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        transport: Arc<dyn Transport>,
        child_pid: Option<u32>,
        wait: Box<dyn FnOnce() -> Exit + Send>,
    ) -> Arc<Self> {
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

        let job_listeners: JobListeners = Arc::new(Mutex::new(Vec::new()));

        // Only when asked, and only when the child actually started: `tcgetpgrp` compares
        // against the shell's own pid, so a spawn that produced no pid has nothing to
        // compare with and watches nothing rather than guessing. A transport with no process
        // group answers `None` for ever, which `JobWatch::observe` reads as no information —
        // see [`Transport::foreground_pgid`].
        let probe = match (spec.watch_jobs, child_pid) {
            (Some(after), Some(pid)) => Some(JobProbe {
                transport: Arc::clone(&transport),
                watch: JobWatch::new(pid as i32, after),
                listeners: Arc::clone(&job_listeners),
                last_poll: None,
            }),
            _ => None,
        };

        spawn_reader(reader, raw_tx);
        spawn_coalescer(
            raw_rx,
            control_rx,
            Arc::clone(&vt),
            Arc::clone(&sinks),
            Arc::clone(&exited),
            spec.credit,
            probe,
            spec.render.clone(),
        );
        spawn_writer(writer, writer_rx);

        let exit = Arc::new(Mutex::new(ExitSlot::Running(Vec::new())));
        spawn_reaper(wait, Arc::clone(&exited), Arc::clone(&exit));

        Arc::new(Self {
            transport,
            writer_tx,
            control_tx,
            vt,
            sinks,
            next_sink_id: AtomicU32::new(1),
            geometry: Mutex::new(spec.geometry),
            fixed_size: spec.fixed_size,
            child_pid,
            exited,
            exit,
            credit: spec.credit,
            job_listeners,
        })
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

    /// Call `f` whenever this pane's foreground goes to a job and comes back.
    ///
    /// Silent unless the session was spawned with [`SpawnSpec::watch_jobs`], which is what
    /// decides whether anything is observed at all; registering here on a session that was
    /// not asked to watch is harmless and simply never fires.
    ///
    /// Runs on the coalescer thread, which is the thread every byte this pane prints passes
    /// through — so `f` must not block, and must not call back into this session's
    /// [`Self::on_job`] (the listener list is held while it runs, and the lock is not
    /// reentrant). Emitting a Tauri event, which is the one caller, does neither.
    pub fn on_job(&self, f: impl Fn(JobEvent) + Send + 'static) {
        self.job_listeners.lock().push(Box::new(f));
    }

    /// Change the job announce threshold on a running session — see
    /// [`SpawnSpec::watch_jobs`], which set it at spawn.
    ///
    /// This exists because the threshold is a user setting now and a session outlives every
    /// pane, tab and window: a threshold fixed at spawn would leave every shell the user
    /// already has open on yesterday's number for the rest of the app's run, which is a
    /// settings row that appears to do nothing. Harmless on a session that watches no jobs —
    /// every Claude pane — where there is no probe for the message to reach.
    ///
    /// Through the control channel rather than a shared atomic, because the coalescer thread
    /// owns the [`JobWatch`] outright and that is worth keeping: the rule's state machine has
    /// exactly one writer, so nothing can interleave with an observation.
    pub fn set_job_announce_after(&self, announce_after: Duration) {
        // A send to a coalescer that has already exited is a session on its way down; there
        // is nobody left to notify about anything, so the error carries no information.
        let _ = self.control_tx.send(Control::JobThreshold(announce_after));
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
    /// The caller is responsible for first delivering [`Self::reattach_state`] so the new
    /// consumer starts from the current screen — on the buffer the child is painting into —
    /// rather than mid-stream.
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
    ///
    /// `history` asks for the mirror's retained scrollback **in front of** the screen, for a
    /// sink that has never seen this session and holds no buffer of its own — a pane opened
    /// onto an agent run that has been printing for twenty minutes (M42). Without it a live
    /// child's snapshot is one screen, and everything above the fold is in the mirror and
    /// nowhere the pane can reach. It is an *option* rather than the default because the
    /// other attaching sinks hold history already: a pane returning from a park keeps its
    /// buffer and refuses the snapshot, and an evicted one replays its own serialized
    /// scrollback first — handing either of them the mirror's copy too would paint the
    /// transcript twice. The caller knows which sink it is; this does not. See
    /// [`history_bytes`] for the composition and why the dead-session arm is unchanged.
    pub fn attach_with_snapshot(&self, sink: Arc<dyn Sink>, history: bool) -> (SinkId, Vec<u8>) {
        let id = self.mint_sink_id();

        if !self.exited.load(Ordering::Acquire) {
            let (reply_tx, reply_rx) = bounded::<Vec<u8>>(1);
            let request = Control::Attach {
                id,
                sink: Arc::clone(&sink),
                history,
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
        // A dead session answers with the whole transcript, not the final screen —
        // [`Self::full_state`]'s doc carries the History argument (a run's twenty minutes
        // shown as its last two dozen lines). Only the *exited* arm: a live child reaching
        // this fallback (a coalescer busy past ATTACH_TIMEOUT) keeps the one-screen
        // `reattach_state`, because a live hydration's scrollback story belongs to the
        // eviction-replay machinery upstream and a full replay here would prepend a second
        // copy of history to a terminal that already holds one.
        if self.exited.load(Ordering::Acquire) {
            return (id, self.full_state());
        }
        // The same answer the coalescer would have given, for the same sink: a caller that
        // asked for history and reached the fallback gets history here too, or the transcript
        // a pane opens with would depend on whether the coalescer was busy at the click.
        if history {
            return (id, self.history_state());
        }
        // `reattach_state`, not `screen_state`: this is still a sink adopting a child's
        // current screen, and the fallback path differing from the coalescer path in *which
        // buffer the receiver ends up on* would make the alt-screen desync depend on whether
        // the coalescer answered in time. See [`reattach_bytes`].
        (id, self.reattach_state())
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
    /// contents, cursor, bracketed paste and mouse protocol modes — and **not** which of the
    /// two buffers that screen is a picture of.
    ///
    /// The doc used to claim it carried the "alt-screen flag" and it never did: this is
    /// `vt100::Screen::state_formatted`, whose `write_contents_formatted` dumps
    /// `self.grid()` — and `grid()` (vt100 0.16.2, `screen.rs`) silently returns whichever of
    /// the normal and alternate grids is active, with nothing said about which. Replaying it
    /// can therefore neither enter nor leave the alternate screen. That omission is
    /// [`Self::reattach_state`]'s to repair, and the two are separate functions rather than
    /// one because the two consumers genuinely want different things:
    ///
    /// * **A picture of a dead child**, replayed into a *fresh* mirror so the user gets their
    ///   last screen back after a restart (`SpawnSpec::preload`, and `cide_app::lifecycle`'s
    ///   `screens.json`). The new child is a fresh shell on the *normal* buffer, so a picture
    ///   that pulled the mirror onto the alternate one would put every byte the new child
    ///   writes into a grid the pane is not showing. This function.
    /// * **A reattach to a child that is still running**, which must land on the buffer the
    ///   child is painting into or the two diverge for the rest of the session.
    ///   [`Self::reattach_state`].
    ///
    /// It is a *screen model* either way, not a byte-exact recorder — OSC 8 hyperlinks,
    /// OSC 52 clipboard traffic and DEC 2026 sync framing are not modelled. For a fullscreen
    /// TUI the caller should follow it with a one-frame `cols-1 → cols` resize nudge so the
    /// application repaints from its own state.
    pub fn screen_state(&self) -> Vec<u8> {
        self.vt.lock().screen().state_formatted()
    }

    /// The whole retained transcript, as flowing lines: every scrollback row the mirror still
    /// holds (oldest first), then the final screen's rows, trailing blanks trimmed.
    ///
    /// [`Self::screen_state`] and [`Self::reattach_state`] both emit **one screen** — `vt100`'s
    /// `contents_formatted` stops at the viewport — and for a live child that is the right
    /// shape: the application owns everything above the fold and repaints on demand. A *dead*
    /// session is the opposite case. Its transcript is the whole reason a pane attaches to it
    /// (the Agents panel's History), the child can repaint nothing, and the one-screen snapshot
    /// showed a twenty-minute run as its last two dozen rendered lines — reported by the first
    /// user to open one as "about 10 lines, why not full session?". The mirror had the answer
    /// all along ([`SCROLLBACK`] lines of it); nothing ever emitted it.
    ///
    /// So this walks the scrollback by stepping `set_scrollback` one row at a time and taking
    /// the top visible row of each window, and emits every row as `<row>\x1b[m\r\n` — the SGR
    /// reset so one row's trailing colour cannot bleed into the next, the hard break costing a
    /// wrapped line its wrap flag and nothing visible. The final screen is emitted the same
    /// way rather than as a `contents_formatted` dump, because that dump leads with a
    /// clear-screen which would erase the just-replayed tail out of the receiving viewport —
    /// the eviction-replay path upstream survives its own such clear only because what it
    /// erases there is exactly what the dump repaints, and here it would not be.
    ///
    /// The lead-in matches [`reattach_bytes`]: ST first, to abort any string sequence the
    /// receiver may be sitting in, then the *normal* buffer selected — scrollback lives there,
    /// and a receiver left on the alternate screen would paint the history into a grid that
    /// never scrolls.
    pub fn full_state(&self) -> Vec<u8> {
        let mut vt = self.vt.lock();
        let (_, cols) = vt.screen().size();
        let mut out = Vec::new();
        out.extend_from_slice(b"\x1b\\\x1b[?1049l");
        scrollback_rows(&mut vt, cols, &mut out);
        // A dead screen's tail is mostly empty rows; replaying them would put a page of blank
        // lines under the transcript. `rows` (plain) decides emptiness, `rows_formatted`
        // supplies what is actually written, and the two iterate the same grid.
        let keep = vt
            .screen()
            .rows(0, cols)
            .collect::<Vec<_>>()
            .iter()
            .rposition(|row| !row.trim().is_empty())
            .map_or(0, |last| last + 1);
        let screen = vt.screen();
        for (i, row) in screen.rows_formatted(0, cols).take(keep).enumerate() {
            out.extend_from_slice(&row);
            out.extend_from_slice(b"\x1b[m");
            // The same join as `scrollback_rows`; the last kept row always ends the line.
            let last = i + 1 == keep;
            if last || !screen.row_wrapped(i as u16) {
                out.extend_from_slice(b"\r\n");
            }
        }
        out
    }

    /// [`Self::screen_state`] made self-sufficient: the buffer identity first, then the
    /// screen. **This is the reattach primitive**, and every sink that is being handed the
    /// current screen of a *live* child gets this one.
    ///
    /// See [`reattach_bytes`] for what it prepends and the bug that made it necessary.
    pub fn reattach_state(&self) -> Vec<u8> {
        reattach_bytes(&self.vt)
    }

    /// [`Self::reattach_state`] with the retained scrollback in front of it — what a sink that
    /// asked for `history` is handed. See [`history_bytes`].
    pub fn history_state(&self) -> Vec<u8> {
        history_bytes(&self.vt)
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
    ///
    /// A resize to the size the child already has is dropped on the floor, and that guard is
    /// worth more than its one line suggests. The body below is the expensive one in this file:
    /// `vt100::Screen::set_size` reflows the whole scrollback, and `cide-app`'s `session_resize`
    /// runs it **synchronously on the thread that receives IPC messages** — deliberately, because
    /// the three locks here are atomic only while callers are serialised (see that command's doc
    /// comment). So a redundant call is not merely wasted work; it is wasted work in front of the
    /// user's next keystroke.
    ///
    /// The frontend already suppresses most of them, but its cache is cleared by four separate
    /// writers — a pane parking, a pane being released to another window, an attach, a font
    /// change — so it cannot cover every call, and every window has its own. This is the backstop
    /// underneath all of them.
    ///
    /// It changes nothing for the callers that resize unconditionally. `session_attach` pushes a
    /// size the child already has, which is exactly the case worth skipping; and the alt-screen
    /// repaint nudge goes `cols - 1` and then `cols`, so both of its steps genuinely change the
    /// value and both still land.
    pub fn resize(&self, geometry: Geometry) -> Result<(), PtyError> {
        // A line-printing child keeps its spawn geometry for life — see `SpawnSpec::fixed_size`
        // for why a resize here would only damage the mirror.
        if self.fixed_size || *self.geometry.lock() == geometry {
            return Ok(());
        }
        self.transport.resize(geometry)?;
        self.vt
            .lock()
            .screen_mut()
            .set_size(geometry.rows, geometry.cols);
        *self.geometry.lock() = geometry;
        Ok(())
    }

    /// Ask the child to exit. Escalation to SIGKILL is the caller's policy.
    pub fn kill(&self) {
        self.transport.kill();
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

/// The mirror's screen, preceded by which of the terminal's two buffers it is a picture of.
///
/// # The bug this exists to prevent
///
/// `vt100::Screen::state_formatted` writes the contents of whichever grid is active and says
/// nothing about which one it was — `write_contents_formatted` walks `self.grid()`, and
/// `grid()` picks between the normal and alternate grids with no marker in the output; the
/// only modes `write_input_mode_formatted` emits are application keypad, application cursor,
/// bracketed paste and the mouse protocol and its encoding. So the snapshot could neither
/// enter nor leave the alternate screen, in either direction, and every consumer of it took
/// it raw.
///
/// That is not cosmetic. A sink that missed the child's own `\x1b[?1049h`/`\x1b[?1049l` — and
/// missing bytes is *designed* behaviour here, see [`broadcast`]: a choked sink has raw
/// frames skipped and is handed this snapshot instead — then sat on a different buffer from
/// the child for the rest of the session, with nothing to bring the two back. Both directions
/// of the desync are user-visible and were both in one bug report:
///
/// * **xterm on the alternate buffer, child on the normal one.** The alternate buffer has no
///   scrollback, so the pane loses its scrollbar and xterm turns the wheel into cursor keys
///   and sends them to the child — the user's wheel starts editing the agent's prompt instead
///   of scrolling its output.
/// * **xterm on the normal buffer, child on the alternate one.** The child's cursor-addressed
///   repaints land in rows that have since scrolled away, so a question the agent drew is
///   never seen; resizing the pane forces a full repaint and it "fixes itself", which is what
///   made this look like a rendering bug rather than a state one.
///
/// # Why prepending is safe unconditionally
///
/// Both `\x1b[?1049h` and `\x1b[?1049l` are guarded no-ops in a terminal that is already on
/// the buffer they name (xterm's `BufferSet.activateAltBuffer`/`activateNormalBuffer` return
/// early when the requested buffer is already active), and the cursor save/restore they carry
/// is immediately overwritten: `state_formatted` opens with `\x1b[m\x1b[H\x1b[J` and closes
/// with an absolute cursor position, both of which are unconditional.
///
/// The leading `\x1b\\` (ST) is for the splice. A catch-up frame is substituted for a raw
/// frame that was *skipped*, so the bytes the receiving parser last saw may have opened an
/// OSC or DCS whose terminator was in a skipped frame — and a parser sitting in a string
/// state would swallow this entire snapshot as payload and paint nothing. ST closes any
/// pending string sequence, aborts a half-written CSI (any `ESC` does), and is ignored in the
/// ground state, so it costs two bytes and can only help.
fn reattach_bytes(vt: &Mutex<vt100::Parser>) -> Vec<u8> {
    let vt = vt.lock();
    let screen = vt.screen();
    let mut out = Vec::new();
    out.extend_from_slice(b"\x1b\\");
    out.extend_from_slice(if screen.alternate_screen() {
        b"\x1b[?1049h"
    } else {
        b"\x1b[?1049l"
    });
    out.extend_from_slice(&screen.state_formatted());
    out
}

/// Every scrollback row the mirror still holds, oldest first, each as `<row>\x1b[m\r\n`.
///
/// The walk [`PtySession::full_state`] has always done, shared with [`history_bytes`] so the two
/// cannot disagree about a row: `set_scrollback` one row at a time, the top visible row of each
/// window, the SGR reset so one row's trailing colour cannot bleed into the next. Leaves the
/// scrollback offset at zero. Answers how many rows it wrote, which is how a caller learns the
/// depth without a second API — the crate clamps `set_scrollback` to what is actually held.
fn scrollback_rows(vt: &mut vt100::Parser, cols: u16, out: &mut Vec<u8>) -> usize {
    vt.screen_mut().set_scrollback(usize::MAX);
    let depth = vt.screen().scrollback();
    for offset in (1..=depth).rev() {
        vt.screen_mut().set_scrollback(offset);
        let screen = vt.screen();
        if let Some(row) = screen.rows_formatted(0, cols).next() {
            out.extend_from_slice(&row);
        }
        out.extend_from_slice(b"\x1b[m");
        // A row the mirror wrapped continues on the next one: no break, so the receiving
        // terminal sees one logical line and soft-wraps it at its own width. (M42) This is what
        // "the hard break costing a wrapped line its wrap flag" used to cost — every line
        // longer than the mirror's width came back as fixed-width fragments in a pane that had
        // room for it. Only rows the mirror still holds a flag for: `vt100` clears every flag
        // on a resize, which is why a line-printing child's mirror is never resized
        // (`SpawnSpec::fixed_size`).
        if !screen.row_wrapped(0) {
            out.extend_from_slice(b"\r\n");
        }
    }
    vt.screen_mut().set_scrollback(0);
    depth
}

/// [`reattach_bytes`] with the retained scrollback in front of it. (M42)
///
/// The composition, in order, and each part is load-bearing:
///
/// 1. ST, then the **normal** buffer — [`PtySession::full_state`]'s lead-in, for its reason:
///    scrollback lives there, and a receiver left on the alternate screen would paint the
///    history into a grid that never scrolls.
/// 2. Every scrollback row, as [`scrollback_rows`] writes them.
/// 3. **One `\r\n` per screen row**, when anything was replayed. The rows just written fill the
///    receiver's viewport from the top, and the dump in step 4 opens with a clear-screen
///    (`\x1b[H\x1b[J`, unconditional in `state_formatted`) that erases the viewport in place
///    rather than scrolling it away — so without this padding the last screenful of history
///    would be wiped the instant it was painted. The newlines push it up into the receiver's
///    scrollback and leave a blank viewport, which is exactly what the dump then paints over.
/// 4. [`reattach_bytes`]: the buffer identity, then the live screen with its cursor and modes,
///    so the child's next byte lands where the child thinks the cursor is.
///
/// A child on the **alternate** screen gets step 4 alone. `vt100` keeps the scrollback on the
/// normal grid and reads through whichever grid is active, so there is nothing to replay while
/// the alternate one is up — and padding the normal buffer with blank rows for nothing would
/// leave a page of empty scrollback behind the moment the TUI exits.
///
/// The dead-session arm ([`PtySession::full_state`]) is deliberately not this function: a dead
/// child has no cursor to restore and its dump would erase the tail, which is why that arm emits
/// the final screen as rows rather than as a dump and needs no padding.
fn history_bytes(mirror: &Mutex<vt100::Parser>) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut vt = mirror.lock();
        let (rows, cols) = vt.screen().size();
        if !vt.screen().alternate_screen() {
            out.extend_from_slice(b"\x1b\\\x1b[?1049l");
            if scrollback_rows(&mut vt, cols, &mut out) > 0 {
                for _ in 0..rows {
                    out.extend_from_slice(b"\r\n");
                }
            }
        }
    }
    // Locked again rather than reusing the guard above: `reattach_bytes` takes the mutex, and
    // on the coalescer thread — the only writer to the mirror — nothing can slip in between.
    out.extend_from_slice(&reattach_bytes(mirror));
    out
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
    probe: &mut Option<JobProbe>,
) {
    match request {
        Control::Attach {
            id,
            sink,
            history,
            reply,
        } => {
            if !pending.is_empty() {
                broadcast(sinks, vt, policy, Frame::Bytes(std::mem::take(pending)));
                *first_byte_at = None;
            }
            sinks.lock().push(registered(id, sink));
            // A caller that has given up (see `ATTACH_TIMEOUT`) leaves nobody on the other
            // end. The sink stays attached regardless — it is registered and will receive
            // output; only the atomicity of its first frame was lost.
            let _ = reply.send(if history {
                history_bytes(vt)
            } else {
                reattach_bytes(vt)
            });
        }
        Control::JobThreshold(after) => {
            // Absent for every session that watches no jobs — a Claude pane — where the
            // retune has nothing to reach and correctly changes nothing.
            if let Some(probe) = probe.as_mut() {
                probe.watch.set_announce_after(after);
            }
        }
    }
}

/// How often the foreground process group is asked about.
///
/// One `tcgetpgrp` per quarter second per watched pane, and only for shells. Slow enough to
/// be free, fast enough that the gap between a job ending and the pane saying so is shorter
/// than the time it takes to look at the screen. It is deliberately the same order as the
/// coalescer's own idle timeout, so a quiet pane's poll rides a wakeup that already happens.
const JOB_POLL_INTERVAL: Duration = Duration::from_millis(250);

/// The observing half of [`crate::jobs`]: the syscall, the clock and the fan-out.
///
/// Lives on the coalescer thread rather than a thread of its own, on `watch_for_exit`'s
/// argument — a wakeup per session per interval, for every pane, is what the exit watcher was
/// rewritten to stop doing. The coalescer is already awake for output and already ticks while
/// idle, so this costs one syscall on a tick that was happening anyway.
struct JobProbe {
    transport: Arc<dyn Transport>,
    watch: JobWatch,
    listeners: JobListeners,
    last_poll: Option<Instant>,
}

impl JobProbe {
    /// Ask, at most once per [`JOB_POLL_INTERVAL`], and report what the rule makes of it.
    ///
    /// Rate-limited here rather than by only polling on the idle tick, and that is not a
    /// refinement: under continuous output — a build printing steadily — the idle arm may
    /// never be selected at all, so a job that both started *and* finished inside one noisy
    /// stretch would be missed entirely. Polling from the output arm too is what makes the
    /// observation independent of how talkative the job is.
    fn poll(&mut self, vt: &Arc<Mutex<vt100::Parser>>, now: Instant) {
        if let Some(last) = self.last_poll
            && now.saturating_duration_since(last) < JOB_POLL_INTERVAL
        {
            return;
        }
        self.last_poll = Some(now);

        // Both locks are taken and released one at a time, never nested: the coalescer holds
        // `vt` on every chunk of output and the pty transport holds `master` for the whole of
        // `foreground_pgid`, so a routine that wanted both at once would be the only place in
        // this file able to order them wrongly.
        let foreground = self.transport.foreground_pgid();
        let alternate_screen = vt.lock().screen().alternate_screen();

        let Some(event) = self.watch.observe(foreground, alternate_screen, now) else {
            return;
        };
        for listener in self.listeners.lock().iter() {
            listener(event);
        }
    }
}

/// How many bytes of one unterminated line [`render_lines`] will hold before giving up on it.
///
/// Generous, because a harness event line legitimately carries a whole file read in one JSON
/// document; what this bounds is a child that simply never writes a newline, whose buffer would
/// otherwise grow without limit. Past it the held bytes are flushed through **raw** — ugly
/// beats lost — and the rest of that line passes unrendered until its terminator arrives.
const RENDER_LINE_CAP: usize = 1 << 20;

/// The partial line the renderer is carrying between chunks.
#[derive(Default)]
struct RenderState {
    buffer: Vec<u8>,
    /// The tail of a line whose head was flushed raw at [`RENDER_LINE_CAP`] — rendering the
    /// tail alone would hand the hook half a document and call it a line.
    resyncing: bool,
}

/// Apply the hook to every complete line in `chunk`, holding the remainder.
///
/// The output substitutes for the raw chunk in the coalescer's stream, so its line endings are
/// written as `\r\n` explicitly: these bytes never pass the pty's own output post-processing
/// (the child's did, which is why the split below strips a trailing `\r` before the hook sees
/// the line).
fn render_lines(state: &mut RenderState, render: &LineRender, chunk: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(chunk.len());
    state.buffer.extend_from_slice(chunk);

    let mut start = 0;
    while let Some(offset) = state.buffer[start..].iter().position(|b| *b == b'\n') {
        let end = start + offset;
        // Two views of the same line: `raw` is what arrived, terminator included, and is what
        // every path that is *not* replacing the line emits, byte for byte. `line` is what the
        // hook is shown — the terminator, and a `\r` before it, are plumbing rather than text.
        let raw = &state.buffer[start..=end];
        let line = &state.buffer[start..end];
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if state.resyncing {
            // The head of this line already went out raw; send the tail the same way and
            // resume rendering from the next line.
            out.extend_from_slice(raw);
            state.resyncing = false;
        } else {
            match render.line(&String::from_utf8_lossy(line)) {
                Rendered::Keep => out.extend_from_slice(raw),
                Rendered::Drop => {}
                Rendered::Replace(text) => {
                    // The hook's text is display content; its own newlines need the full
                    // terminator too, for the reason in the function doc.
                    out.extend_from_slice(text.replace('\n', "\r\n").as_bytes());
                    out.extend_from_slice(b"\r\n");
                }
            }
        }
        start = end + 1;
    }
    state.buffer.drain(..start);

    // What is left is an unterminated tail, and holding it is the whole hazard: for an
    // interactive child the bytes with no newline are the prompt. Ask the hook whether they
    // could still become a line it would rewrite, and flush them raw when they could not —
    // or when they have grown past what any one line can be worth.
    if !state.buffer.is_empty()
        && (!render.holds(&state.buffer) || state.buffer.len() > RENDER_LINE_CAP)
    {
        flush_tail(state, &mut out);
    }
    out
}

/// Send the held partial line out raw and remember that its head has gone.
///
/// `resyncing` is the memory: the remainder of that line arrives later with a terminator on
/// it, and handing *that* to the hook would be handing it half a document and calling it a
/// line — which for a JSON renderer means a parse failure on text that was never malformed.
fn flush_tail(state: &mut RenderState, out: &mut Vec<u8>) {
    out.append(&mut state.buffer);
    state.resyncing = true;
}

#[allow(clippy::too_many_arguments)] // one private call site; a params struct would name nothing
fn spawn_coalescer(
    rx: Receiver<Vec<u8>>,
    control_rx: Receiver<Control>,
    vt: Arc<Mutex<vt100::Parser>>,
    sinks: Arc<Mutex<Vec<Registered>>>,
    exited: Arc<AtomicBool>,
    policy: CreditPolicy,
    mut probe: Option<JobProbe>,
    render: Option<LineRender>,
) {
    thread::Builder::new()
        .name("cide-pty-coalesce".into())
        .spawn(move || {
            let mut pending: Vec<u8> = Vec::with_capacity(FLUSH_BYTES * 2);
            let mut first_byte_at: Option<Instant> = None;
            let mut render_state = RenderState::default();

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
                            &mut probe,
                        );
                    }
                    Event::Output(chunk) => {
                        // The render hook, if any, rewrites the stream **here** — before the
                        // mirror, before `pending`, before the frame splitter — so everything
                        // downstream, the choked-sink catch-up included, agrees on one text.
                        // See [`LineRender`]. A chunk that ended mid-line renders to nothing
                        // and is held; skipping the rest keeps `first_byte_at` honest.
                        let chunk = match render.as_ref() {
                            Some(render) => render_lines(&mut render_state, render, &chunk),
                            None => chunk,
                        };
                        if chunk.is_empty() {
                            continue;
                        }
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
                        // After the mirror has the chunk, so the alternate-screen bit the
                        // rule reads is this instant's rather than the previous poll's.
                        if let Some(probe) = probe.as_mut() {
                            probe.poll(&vt, Instant::now());
                        }
                    }
                    Event::Idle => {
                        // A held partial line that has gone quiet. Only an interactive
                        // renderer holds one speculatively (see `LineRender::partial`), and
                        // for that shape a quarter second of silence settles the question:
                        // nothing is going to complete these bytes, they are a prompt that
                        // happens to begin like a document, and holding them any longer is a
                        // pane that looks wedged. Before the flush below, so they leave in
                        // this tick rather than waiting for the next byte the child writes.
                        if let Some(render) = render.as_ref()
                            && render.flushes_when_idle()
                            && !render_state.buffer.is_empty()
                        {
                            let mut flushed = Vec::new();
                            flush_tail(&mut render_state, &mut flushed);
                            vt.lock().process(&flushed);
                            pending.extend_from_slice(&flushed);
                        }
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
                        // The arm that carries a *finished* job: work ends, the shell prints
                        // a prompt, the pane goes quiet, and this fires a quarter second
                        // later. The output arm above covers the noisy half.
                        if let Some(probe) = probe.as_mut() {
                            probe.poll(&vt, Instant::now());
                        }
                    }
                    Event::Eof => {
                        // A final line the child never terminated still belongs on screen —
                        // rendered stream or not, EOF is the one point after which nothing
                        // else will ever complete it.
                        if !render_state.buffer.is_empty() {
                            vt.lock().process(&render_state.buffer);
                            pending.extend_from_slice(&render_state.buffer);
                            render_state.buffer.clear();
                        }
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
            catchup.get_or_insert_with(|| reattach_bytes(vt))
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
    wait: Box<dyn FnOnce() -> Exit + Send>,
    exited: Arc<AtomicBool>,
    slot: Arc<Mutex<ExitSlot>>,
) {
    thread::Builder::new()
        .name("cide-pty-reap".into())
        .spawn(move || {
            let exit = wait();
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

    /// A real `bash`, a real job, and the two events a pane needs to notify.
    ///
    /// The unit tests in [`crate::jobs`] drive the rule; this drives the half they cannot —
    /// that `tcgetpgrp` on this pty actually changes when a shell runs something, which is
    /// the entire premise of the feature and is a claim about the platform rather than about
    /// any code here.
    ///
    /// `--norc -i`: interactive, because job control is what puts a job in a process group of
    /// its own and bash only turns it on for an interactive shell — a `bash -c` never leaves
    /// its own group and this would (correctly) observe nothing. `--norc` so the test does not
    /// source whatever is in the developer's rc files.
    #[cfg(unix)]
    #[test]
    fn a_real_shell_reports_a_job_starting_and_finishing() {
        let spec = SpawnSpec::new("/bin/bash", std::env::temp_dir())
            .arg("--norc")
            .arg("-i")
            // Far below the two minutes the app defaults to, for the ordinary reason: a test
            // that waits out a production threshold is a test nobody runs.
            .watch_jobs(Duration::from_millis(100));
        let session = PtySession::spawn(spec).expect("spawn bash");

        let (tx, rx) = mpsc::channel::<JobEvent>();
        session.on_job(move |event| {
            let _ = tx.send(event);
        });

        // A sink, so the pane is being read exactly as a real one is. Without it the
        // coalescer still runs, but keeping the shape honest costs one line.
        let (sink, _out) = collect_sink();
        session.attach(sink);

        // Let bash reach its first prompt before asking it to do anything: the rule
        // deliberately says nothing until it has seen the shell own its own terminal, which
        // is the guard against a slow `.bash_profile` being reported as the pane's first job.
        thread::sleep(Duration::from_millis(400));
        session.write(b"sleep 1\n".to_vec());

        let started = rx
            .recv_timeout(DEADLINE)
            .expect("a job that outran the threshold is announced");
        assert_eq!(started, JobEvent::Started);

        let finished = rx
            .recv_timeout(DEADLINE)
            .expect("and the prompt coming back ends it");
        match finished {
            JobEvent::Finished { ran_for } => {
                assert!(
                    ran_for >= Duration::from_millis(500),
                    "the duration is the job's, not the poll interval's: {ran_for:?}",
                );
            }
            other => panic!("expected a finished job, got {other:?}"),
        }
    }

    /// The pane every user already has: no watching asked for, nothing observed, no events.
    ///
    /// Pinned because the cost of getting this wrong is not a crash — it is every Claude pane
    /// in the app publishing a second, contradictory opinion about its own `SessionState`
    /// every time the CLI forks a tool.
    #[cfg(unix)]
    #[test]
    fn a_session_that_did_not_ask_to_be_watched_reports_nothing() {
        let spec = SpawnSpec::new("/bin/bash", std::env::temp_dir())
            .arg("--norc")
            .arg("-i");
        let session = PtySession::spawn(spec).expect("spawn bash");

        let (tx, rx) = mpsc::channel::<JobEvent>();
        session.on_job(move |event| {
            let _ = tx.send(event);
        });
        let (sink, _out) = collect_sink();
        session.attach(sink);

        thread::sleep(Duration::from_millis(400));
        session.write(b"sleep 1\n".to_vec());
        assert!(
            rx.recv_timeout(Duration::from_secs(3)).is_err(),
            "an unwatched session announced a job",
        );
    }

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

    /// A line-printing child's mirror keeps its spawn width for life, and a line it wrapped is
    /// replayed as one line. (M42)
    ///
    /// Both halves of the "text is not on full width" report: a resize would truncate the wide
    /// mirror and clear its wrap flags (`vt100` does not reflow), and a hard break at every
    /// mirror row is what turned a 300-character rendered line into fragments in a pane that
    /// had room for it.
    #[test]
    fn a_fixed_size_mirror_ignores_resizes_and_replays_wrapped_rows_joined() {
        let long = "x".repeat(150);
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg(format!("printf '%s\\n' first {long} last"))
            .geometry(Geometry::new(100, 24, 8, 16))
            .fixed_size();
        let session = PtySession::spawn(spec).expect("spawn sh");
        let deadline = Instant::now() + DEADLINE;
        while !session.has_exited() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(session.has_exited());

        // The resize a pane would make on attach is refused, so the width stays at 100 and the
        // 150-character row keeps its wrap flag.
        session
            .resize(Geometry::new(40, 10, 8, 16))
            .expect("a refused resize is not an error");
        assert_eq!(session.geometry().cols, 100);

        // The SGR reset between two rows of one line is deliberate (a colour must not bleed
        // from one mirror row into the next); what must be absent is the line break.
        let replay = String::from_utf8_lossy(&session.full_state())
            .into_owned()
            .replace("\x1b[m", "");
        assert!(
            replay.contains(&long),
            "the wrapped row is replayed as one logical line: {replay:?}"
        );
        assert!(
            replay.contains("first") && replay.contains("last"),
            "{replay:?}"
        );
    }

    /// A sink that asks for history is handed the scrollback in front of the live screen; one
    /// that does not gets the one screen it always got. (M42)
    ///
    /// A hundred rows into a 24-row mirror: the first rows have long scrolled off, and the
    /// child is still alive (`sleep`), so this is the coalescer arm and not the dead-session
    /// fallback that always replayed everything.
    #[test]
    fn a_sink_asking_for_history_is_handed_the_scrollback_before_the_screen() {
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("i=1; while [ $i -le 100 ]; do echo row-$i-end; i=$((i+1)); done; sleep 30")
            .geometry(Geometry::new(80, 24, 8, 16));
        let session = PtySession::spawn(spec).expect("spawn sh");

        // Wait on the mirror, for the reason the test above gives.
        let deadline = Instant::now() + DEADLINE;
        loop {
            let screen = String::from_utf8_lossy(&session.screen_state()).into_owned();
            if screen.contains("row-100-end") {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the child never printed its last row: {screen:?}"
            );
            thread::sleep(Duration::from_millis(10));
        }

        let (_, plain) = session.attach_with_snapshot(Arc::new(|_: &[u8]| true), false);
        let plain = String::from_utf8_lossy(&plain).into_owned();
        assert!(
            plain.contains("row-100-end"),
            "the live screen is missing: {plain:?}"
        );
        assert!(
            !plain.contains("row-1-end"),
            "a plain attach must stay one screen — the eviction replay upstream owns history: {plain:?}"
        );

        let (_, with_history) = session.attach_with_snapshot(Arc::new(|_: &[u8]| true), true);
        let text = String::from_utf8_lossy(&with_history).into_owned();
        let first = text
            .find("row-1-end")
            .expect("the first row is in the history");
        let clear = text
            .find("\x1b[H\x1b[J")
            .expect("the screen dump's clear-screen");
        let last = text.rfind("row-100-end").expect("the live screen follows");
        assert!(
            first < clear && clear < last,
            "history, then the dump, then the screen: {text:?}"
        );
        // The padding that keeps the dump's clear from erasing the replayed tail: one line
        // break per screen row, between the last history row and the dump.
        let between = &text[first..clear];
        assert!(
            between.matches("\r\n").count() >= 24,
            "the replayed tail must be pushed past the viewport before the clear: {between:?}"
        );
        assert!(
            text.contains("\x1b[?1049l"),
            "history is replayed onto the normal buffer: {text:?}"
        );

        session.kill();
    }

    /// The line splitter under the render hook: chunks land mid-line, terminators vary, the
    /// hook may drop or multiply lines, and the cap flushes raw rather than losing bytes.
    #[test]
    fn render_lines_holds_partials_and_flushes_the_oversized_raw() {
        let render = LineRender::new(Arc::new(|line: &str| {
            if line == "drop" {
                return Rendered::Drop;
            }
            Rendered::Replace(format!("<{line}>"))
        }));
        let mut state = RenderState::default();

        // A line split across two chunks renders once, when it completes; `\r\n` arrives as
        // the pty wrote it and the hook sees neither half of the terminator.
        assert_eq!(render_lines(&mut state, &render, b"hel"), b"");
        assert_eq!(
            render_lines(&mut state, &render, b"lo\r\nwo"),
            b"<hello>\r\n"
        );
        assert_eq!(render_lines(&mut state, &render, b"rld\n"), b"<world>\r\n");

        // `Drop` removes the line from the display; a multi-line rendering gets real
        // terminators.
        assert_eq!(render_lines(&mut state, &render, b"drop\n"), b"");
        let two = LineRender::new(Arc::new(|_: &str| Rendered::Replace("a\nb".into())));
        assert_eq!(render_lines(&mut state, &two, b"x\n"), b"a\r\nb\r\n");

        // Past the cap the held head goes out raw — ugly beats lost — and the tail of that
        // line follows raw at its terminator, with rendering resuming on the next line.
        let mut state = RenderState::default();
        let huge = vec![b'x'; RENDER_LINE_CAP + 1];
        assert_eq!(render_lines(&mut state, &render, &huge), huge);
        assert!(state.resyncing);
        assert_eq!(
            render_lines(&mut state, &render, b"tail\nok\n"),
            b"tail\n<ok>\r\n"
        );
    }

    /// `Keep` is byte-identical to having installed no hook at all — terminator included.
    ///
    /// The terminator is the whole assertion. A `Keep` spelled "render the line back as
    /// itself" would re-terminate it `\r\n`, and a program in raw mode emits a bare `\n`
    /// meaning *down one row, same column*: turning that into CRLF walks every line of a
    /// fullscreen TUI back to column 0. Nothing throws, nothing logs, and it happens only to
    /// people running `vim` in a pane.
    #[test]
    fn a_kept_line_keeps_its_own_bytes() {
        let render = LineRender::new(Arc::new(|_: &str| Rendered::Keep));
        let mut state = RenderState::default();
        for chunk in [
            &b"bare\nlf\n"[..],
            &b"crlf\r\n"[..],
            &b"\x1b[2K\x1b[Ktui\n"[..],
            &b"\r\n"[..],
        ] {
            assert_eq!(render_lines(&mut state, &render, chunk), chunk);
        }
    }

    /// An unterminated tail is held only while the hook says it could still become a line.
    ///
    /// This is what makes the hook safe on an interactive child. A shell prompt has no
    /// newline, and under the unconditional hold it is invisible until the user presses
    /// Enter — a pane that takes keystrokes and shows nothing, which reads as a hang.
    #[test]
    fn an_uninteresting_tail_is_not_held() {
        let render = LineRender::new(Arc::new(|line: &str| {
            Rendered::Replace(format!("<{line}>"))
        }))
        .holding_only(Arc::new(|tail: &str| tail.trim_start().starts_with('{')));
        let mut state = RenderState::default();

        // The prompt leaves in the same chunk it arrived in, byte for byte.
        assert_eq!(
            render_lines(&mut state, &render, b"user@host:~$ "),
            b"user@host:~$ "
        );
        assert!(
            state.resyncing,
            "its head has gone, so its line is no longer whole"
        );
        // What the user types then echoes back with the terminator, still raw — handing the
        // hook the tail alone would be handing it half a line.
        assert_eq!(render_lines(&mut state, &render, b"ls\r\n"), b"ls\r\n");
        // And the next whole line renders again.
        assert_eq!(render_lines(&mut state, &render, b"a\n"), b"<a>\r\n");

        // A tail that *could* still complete is held, and completing it renders it whole.
        assert_eq!(render_lines(&mut state, &render, b"{\"a\":"), b"");
        assert_eq!(
            render_lines(&mut state, &render, b"1}\n"),
            b"<{\"a\":1}>\r\n"
        );
    }

    /// The hook, end to end: one rendered stream is what the mirror holds and what a sink is
    /// delivered — there is no raw copy anywhere downstream for the two to disagree over.
    ///
    /// # Two races this fixture closes, both of which ended in a sink holding `""`
    ///
    /// The child prints nothing until it is *told to*, by a line the test writes after the sink
    /// is attached: a fresh sink gets no replay from the pty (hydration is the pane layer's
    /// job), and under a saturated box the reader thread once delivered the whole output before
    /// `attach` ran. A `sleep 0.2` before the `printf` guarded that for a while; a gate the test
    /// holds guards it by construction.
    ///
    /// And the sink is waited on **directly**, not read the moment the mirror shows the last
    /// line. The output arm feeds the mirror as each chunk arrives and hands sinks the bytes
    /// only at the next flush — a small chunk waits for the idle tick, up to `FLUSH_INTERVAL`
    /// later (`PtySession::attach_with_snapshot`'s doc states the lag) — so the mirror
    /// legitimately runs ahead of every sink by a few milliseconds. Polling the mirror and then
    /// asserting on the sink was reading the sink inside that window, which a 10 ms poll on a
    /// quiet machine rarely lands in and a loaded 2-vCPU CI runner did: the coalescer thread
    /// was preempted between the mirror feed and the flush, and the assertion read `""`. The
    /// second failure carried the first's diagnosis in its message, which is why it took two
    /// to see.
    #[test]
    fn a_rendered_session_shows_the_rendering_in_mirror_and_sink_alike() {
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("read go; printf 'alpha\\nbeta\\n'")
            .render(LineRender::new(Arc::new(|line: &str| {
                Rendered::Replace(format!("[{line}]"))
            })));
        let session = PtySession::spawn(spec).expect("spawn sh");

        let received: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_copy = Arc::clone(&received);
        session.attach(Arc::new(move |bytes: &[u8]| -> bool {
            sink_copy.lock().extend_from_slice(bytes);
            true
        }));
        // The gate. Everything the child prints from here is printed to an attached sink.
        session.write(b"go\n".to_vec());

        let deadline = Instant::now() + DEADLINE;
        let mut delivered = String::new();
        while Instant::now() < deadline {
            delivered = String::from_utf8_lossy(&received.lock()).into_owned();
            if delivered.contains("[beta]") {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(
            delivered.contains("[alpha]") && delivered.contains("[beta]"),
            "a sink was delivered something other than the rendering: {delivered:?}"
        );
        assert!(
            !delivered.contains("alpha\nbeta"),
            "the raw lines leaked past the hook: {delivered:?}"
        );

        // By the time a sink holds a byte the mirror already held it — the mirror is fed first
        // and only from the coalescer thread — so this needs no wait of its own.
        let screen = String::from_utf8_lossy(&session.screen_state()).into_owned();
        assert!(
            screen.contains("[alpha]") && screen.contains("[beta]"),
            "the mirror holds the rendered stream, so a rehydrated pane would too: {screen:?}"
        );
        assert!(
            !screen.contains("alpha\nbeta"),
            "the raw lines leaked past the hook: {screen:?}"
        );
    }

    /// **A dead session's snapshot is the whole transcript, not the final screen.**
    ///
    /// The regression this pins: a run's pane opened from the Agents panel's History showed
    /// "about 10 lines" of a twenty-minute session, because every snapshot path emitted
    /// `contents_formatted` — the viewport — while the mirror sat on 5,000 lines of
    /// scrollback nothing ever read. A 24-row screen over 200 numbered lines makes the loss
    /// mechanical: line 1 exists only in scrollback, so this assertion fails on any
    /// one-screen snapshot.
    #[test]
    fn an_exited_sessions_snapshot_carries_its_whole_scrollback() {
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("i=1; while [ $i -le 200 ]; do echo \"transcript line $i\"; i=$((i+1)); done");
        let session = PtySession::spawn(spec).expect("spawn sh");

        let deadline = Instant::now() + DEADLINE;
        while !session.has_exited() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(session.has_exited(), "child exit was never observed");
        // The exit is reaped before the last output is necessarily processed; the mirror is
        // fed on the coalescer thread, so wait for the tail line to land in it.
        let deadline = Instant::now() + DEADLINE;
        while !String::from_utf8_lossy(&session.screen_state()).contains("transcript line 200")
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(20));
        }

        let (_, snapshot) = session.attach_with_snapshot(Arc::new(|_: &[u8]| true), false);
        let text = String::from_utf8_lossy(&snapshot);
        assert!(
            text.contains("transcript line 1\u{1b}") || text.contains("transcript line 1\r"),
            "the first line lives only in scrollback and must be in the snapshot"
        );
        assert!(text.contains("transcript line 200"), "{}", text.len());
        // And the ordering is the transcript's: line 1 before line 200.
        let first = text.find("transcript line 1").expect("asserted above");
        let last = text.find("transcript line 200").expect("asserted above");
        assert!(first < last, "scrollback precedes the screen");
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
        //
        // The `sleep` is what makes counting from a sink race-free: the sink is attached
        // after the spawn and gets no replay from the pty, and under a saturated box the
        // reader thread once delivered the entire burst before `attach` ran — this test then
        // failed with "0 bytes across 0 frames" after the full deadline, the coalescer
        // blameless. Three load flakes were misread as a slow burst before the attach race
        // was recognised; the rendered-session test above carries the same beat for the same
        // reason.
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("sleep 0.2; for i in $(seq 1 2000); do printf x; done");
        let session = PtySession::spawn(spec).expect("spawn sh");

        let (tx, rx) = mpsc::channel::<usize>();
        let tx = Mutex::new(tx);
        let sink: Arc<dyn Sink> = Arc::new(move |b: &[u8]| tx.lock().send(b.len()).is_ok());
        session.attach(sink);

        // Three deadlines, not one: under a full `--workspace` run this box is saturated and
        // the child's own 2000-iteration loop can take longer than the shared DEADLINE — the
        // coalescer blameless both times this test has flaked on load. The property under test
        // is the frames-to-bytes ratio, which waiting longer cannot fake.
        let deadline = Instant::now() + DEADLINE * 3;
        let mut frames = 0usize;
        let mut total = 0usize;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            // A timeout keeps waiting until the byte count is in or the deadline is. This
            // used to break on any 400 ms silence, which read "the burst is over" — and
            // under a full `--workspace` run the shell's own loop stalls longer than that
            // mid-burst, so the test failed on machine load with the coalescer blameless.
            // The comment under the next test names this exact class.
            let Ok(n) = rx.recv_timeout(remaining.min(Duration::from_millis(400))) else {
                continue;
            };
            frames += 1;
            total += n;
            if total >= 2000 {
                break;
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

    /// The reattach primitive must be able to *enter* the alternate screen.
    ///
    /// `state_formatted()` alone cannot: it dumps whichever grid is active and emits no
    /// `?1049`, so replaying an alt-screen snapshot into a fresh terminal left that terminal
    /// on the normal buffer while the child kept painting the alternate one. The child's
    /// cursor-addressed repaints then landed in rows that had scrolled away, which is
    /// "Claude renders a question and I never see it".
    #[test]
    fn an_alt_screen_snapshot_puts_a_fresh_terminal_on_the_alternate_screen() {
        let vt = Mutex::new(vt100::Parser::new(24, 80, 100));
        vt.lock().process(b"\x1b[?1049h\x1b[HTUI FRAME");
        assert!(
            vt.lock().screen().alternate_screen(),
            "the fixture never entered the alternate screen"
        );

        let bytes = reattach_bytes(&vt);
        assert!(
            !vt100::Parser::new(24, 80, 100).screen().alternate_screen(),
            "a fresh parser is expected to start on the primary screen"
        );

        let mut fresh = vt100::Parser::new(24, 80, 100);
        fresh.process(&bytes);
        assert!(
            fresh.screen().alternate_screen(),
            "the snapshot did not carry the alternate screen: {:?}",
            String::from_utf8_lossy(&bytes)
        );
        assert!(
            fresh.screen().contents().contains("TUI FRAME"),
            "the snapshot carried the buffer but lost the screen"
        );
    }

    /// And it must be able to *leave* it, which is the direction that loses the scrollbar.
    ///
    /// A terminal stuck on the alternate buffer reports no scrollback, and xterm then turns
    /// the wheel into cursor keys and sends them to the child — the wheel edits the agent's
    /// prompt instead of scrolling its output, which is exactly what was reported.
    #[test]
    fn a_primary_screen_snapshot_brings_an_alt_screen_terminal_back() {
        let vt = Mutex::new(vt100::Parser::new(24, 80, 100));
        vt.lock().process(b"\x1b[HSHELL PROMPT");
        let bytes = reattach_bytes(&vt);

        let mut stuck = vt100::Parser::new(24, 80, 100);
        stuck.process(b"\x1b[?1049h");
        assert!(
            stuck.screen().alternate_screen(),
            "the fixture is not stuck"
        );

        stuck.process(&bytes);
        assert!(
            !stuck.screen().alternate_screen(),
            "the snapshot could not bring a terminal off the alternate screen: {:?}",
            String::from_utf8_lossy(&bytes)
        );
        assert!(
            stuck.screen().contents().contains("SHELL PROMPT"),
            "the snapshot left the terminal on the right buffer with the wrong screen"
        );
    }

    /// The regression test for the reported bug, on the path that actually fires.
    ///
    /// Volume is the trigger: only a *choke* skips raw frames, and a choke that straddles the
    /// child's own `?1049h` is what desynced the buffers. The catch-up frame is the sink's
    /// only route back, so it has to say which buffer the screen it carries belongs to.
    #[test]
    fn a_catch_up_frame_carries_the_buffer_the_sink_missed_the_switch_to() {
        let h = Harness::new(policy(500, 100, Duration::from_secs(3600)));
        let (sink, log) = recording();
        let id = h.attach(sink);

        // Choke it first, so everything after this is skipped rather than delivered.
        h.feed(b"NORMAL SCREEN\r\n");
        for _ in 0..10 {
            h.feed(&vec![b'x'; 400]);
        }
        assert!(h.choked(id), "never choked");

        // The switch the sink must not be allowed to miss, and a frame drawn after it.
        h.feed(b"\x1b[?1049h\x1b[HQUESTION: proceed?");
        let raw_seen = log.lock().concat();
        assert!(
            !String::from_utf8_lossy(&raw_seen).contains("QUESTION"),
            "a choked sink was still being sent raw output"
        );

        h.ack(id, h.outstanding(id));
        h.tick();

        let catchup = log.lock().last().cloned().expect("a catch-up frame");
        let mut pane = vt100::Parser::new(24, 80, 100);
        pane.process(&raw_seen);
        pane.process(&catchup);
        assert!(
            pane.screen().alternate_screen(),
            "a sink that was choked across `?1049h` never reached the alternate screen: {:?}",
            String::from_utf8_lossy(&catchup)
        );
        assert!(
            pane.screen().contents().contains("QUESTION: proceed?"),
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

        let (_, screen) = session.attach_with_snapshot(Arc::new(|_: &[u8]| true), false);
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

    /// A resize to the size the child already has does nothing, and says so.
    ///
    /// Weak by construction, and worth having anyway. What the guard actually buys is the
    /// *absence* of a scrollback reflow on the IPC thread, and nothing observable from here can
    /// assert an absence — so this pins the two things that are observable: it still answers
    /// `Ok`, and it leaves the geometry alone. The argument for the guard is in `resize`'s own
    /// comment; this is what stops someone deleting the early return as dead code.
    #[test]
    fn resizing_to_the_size_it_already_has_is_a_no_op() {
        let spec = SpawnSpec::new("/bin/sh", std::env::temp_dir())
            .arg("-c")
            .arg("sleep 5")
            .geometry(Geometry::new(80, 24, 8, 17));
        let session = PtySession::spawn(spec).expect("spawn sh");
        let before = session.geometry();

        session
            .resize(Geometry::new(80, 24, 8, 17))
            .expect("a no-op resize still succeeds");
        assert_eq!(session.geometry(), before);

        // And the guard is on the *whole* geometry, not on the cell count: a font change moves
        // the pixel size with the same cols and rows, and a child that queries the cell size for
        // sixel or pixel mouse reporting has to be told.
        session
            .resize(Geometry::new(80, 24, 9, 19))
            .expect("resize");
        assert_eq!(session.geometry().cell_width, 9);
        assert_eq!(session.geometry().cell_height, 19);

        session.kill();
    }
}
