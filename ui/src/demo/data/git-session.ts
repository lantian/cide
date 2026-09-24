/**
 * `crates/cide-pty/src/session.rs` three times over — the merge base, this branch's version and
 * master's — plus a small line differ, shared by the `git` scene (its diff tab) and the `merge`
 * scene (its three panes).
 *
 * One file told consistently across scenes, because the Claude panes, the changes tree and the
 * conflict resolver are all about the same change: the coalescer moving out of `session.rs`
 * into `coalesce.rs` and `push` learning to say `Pressure::Hold`. Master meanwhile shortened
 * the flush interval and started counting frames, which is what makes the merge conflict.
 */
import type { DiffHunkView, DiffLineView } from '../../ipc/generated'

/** The merge base: the coalescer still lives inline in the session. */
export const SESSION_BASE = `//! One PTY child and the bytes it writes,
//! fanned out to every pane attached to it.

use std::io::{self, Read};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tracing::{debug, trace};

use crate::vt::Mirror;
use crate::{Geometry, SessionId, Sink};

/// How long bytes wait for company before they
/// are flushed to the webview.
const FLUSH_AFTER: Duration = Duration::from_millis(4);

/// The largest frame one flush may carry.
const MAX_FRAME: usize = 64 * 1024;

pub struct Session {
    id: SessionId,
    mirror: Mutex<Mirror>,
    sinks: Mutex<Vec<Arc<dyn Sink>>>,
    pending: Mutex<Vec<u8>>,
    last_flush: Mutex<Instant>,
}

impl Session {
    pub fn new(id: SessionId, geometry: Geometry) -> Self {
        Self {
            id,
            mirror: Mutex::new(Mirror::new(geometry)),
            sinks: Mutex::new(Vec::new()),
            pending: Mutex::new(Vec::with_capacity(MAX_FRAME)),
            last_flush: Mutex::new(Instant::now()),
        }
    }

    /// Feed the reader's bytes in; flush when the
    /// frame is full or old enough.
    pub fn push(&self, bytes: &[u8]) {
        self.mirror.lock().advance(bytes);
        let mut pending = self.pending.lock();
        pending.extend_from_slice(bytes);
        let due = self.last_flush.lock().elapsed() >= FLUSH_AFTER;
        if pending.len() >= MAX_FRAME || due {
            let frame = std::mem::take(&mut *pending);
            drop(pending);
            self.flush(frame);
        }
    }

    fn flush(&self, frame: Vec<u8>) {
        trace!(session = %self.id, len = frame.len(), "flush");
        for sink in self.sinks.lock().iter() {
            sink.send(&frame);
        }
        *self.last_flush.lock() = Instant::now();
    }

    /// Attach a pane: the mirror's screen now,
    /// and every frame after.
    pub fn attach(&self, sink: Arc<dyn Sink>) -> Vec<u8> {
        debug!(session = %self.id, "attach");
        self.sinks.lock().push(sink);
        self.mirror.lock().snapshot()
    }
}
`

/** `pty-backpressure`: the coalescer split out, `push` answering with a `Pressure`. */
export const SESSION_OURS = `//! One PTY child and the bytes it writes,
//! fanned out to every pane attached to it.

use std::io::{self, Read};
use std::sync::Arc;

use parking_lot::Mutex;
use tracing::{debug, trace};

use crate::coalesce::{Coalescer, Pressure};
use crate::vt::Mirror;
use crate::{Geometry, SessionId, Sink};

pub struct Session {
    id: SessionId,
    mirror: Mutex<Mirror>,
    sinks: Mutex<Vec<Arc<dyn Sink>>>,
    /// The frame being built and the ack window;
    /// see \`coalesce.rs\`.
    coalescer: Mutex<Coalescer>,
}

impl Session {
    pub fn new(id: SessionId, geometry: Geometry) -> Self {
        Self {
            id,
            mirror: Mutex::new(Mirror::new(geometry)),
            sinks: Mutex::new(Vec::new()),
            coalescer: Mutex::new(Coalescer::default()),
        }
    }

    /// Feed the reader's bytes in. \`Hold\` stops
    /// the reader until the webview acks what
    /// it has: buffering without bound is how
    /// \`yes | head -c 2G\` took the window down.
    pub fn push(&self, bytes: &[u8]) -> Pressure {
        self.mirror.lock().advance(bytes);
        let mut coalescer = self.coalescer.lock();
        let pressure = coalescer.push(bytes);
        if let Some(frame) = coalescer.take_due() {
            drop(coalescer);
            self.flush(frame);
        }
        pressure
    }

    fn flush(&self, frame: Vec<u8>) {
        trace!(session = %self.id, len = frame.len(), "flush");
        for sink in self.sinks.lock().iter() {
            sink.send(&frame);
        }
    }

    /// The webview painted \`len\` bytes; reopen
    /// the window by that much.
    pub fn ack(&self, len: usize) {
        self.coalescer.lock().ack(len);
    }

    /// Attach a pane: the mirror's screen now,
    /// and every frame after.
    pub fn attach(&self, sink: Arc<dyn Sink>) -> Vec<u8> {
        debug!(session = %self.id, "attach");
        self.sinks.lock().push(sink);
        self.mirror.lock().snapshot()
    }
}
`

/** `master`: a shorter flush interval and a frame counter, both inside the coalescer's old home. */
export const SESSION_THEIRS = `//! One PTY child and the bytes it writes,
//! fanned out to every pane attached to it.

use std::io::{self, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tracing::{debug, trace};

use crate::vt::Mirror;
use crate::{Geometry, SessionId, Sink};

/// How long bytes wait for company before they
/// are flushed. Two milliseconds: long enough to
/// batch a burst, short enough that typing never
/// feels it.
const FLUSH_AFTER: Duration = Duration::from_millis(2);

/// The largest frame one flush may carry.
const MAX_FRAME: usize = 64 * 1024;

pub struct Session {
    id: SessionId,
    mirror: Mutex<Mirror>,
    sinks: Mutex<Vec<Arc<dyn Sink>>>,
    pending: Mutex<Vec<u8>>,
    last_flush: Mutex<Instant>,
    /// Frames sent, for \`cide-headless sessions\`.
    frames: AtomicU64,
}

impl Session {
    pub fn new(id: SessionId, geometry: Geometry) -> Self {
        Self {
            id,
            mirror: Mutex::new(Mirror::new(geometry)),
            sinks: Mutex::new(Vec::new()),
            pending: Mutex::new(Vec::with_capacity(MAX_FRAME)),
            last_flush: Mutex::new(Instant::now()),
            frames: AtomicU64::new(0),
        }
    }

    /// Feed the reader's bytes in; flush when the
    /// frame is full or old enough.
    pub fn push(&self, bytes: &[u8]) {
        self.mirror.lock().advance(bytes);
        let mut pending = self.pending.lock();
        pending.extend_from_slice(bytes);
        let due = self.last_flush.lock().elapsed() >= FLUSH_AFTER;
        if pending.len() >= MAX_FRAME || due {
            let frame = std::mem::take(&mut *pending);
            drop(pending);
            self.flush(frame);
        }
    }

    fn flush(&self, frame: Vec<u8>) {
        trace!(session = %self.id, len = frame.len(), "flush");
        for sink in self.sinks.lock().iter() {
            sink.send(&frame);
        }
        self.frames.fetch_add(1, Ordering::Relaxed);
        *self.last_flush.lock() = Instant::now();
    }

    /// Attach a pane: the mirror's screen now,
    /// and every frame after.
    pub fn attach(&self, sink: Arc<dyn Sink>) -> Vec<u8> {
        let sinks = self.sinks.lock().len();
        debug!(session = %self.id, sinks, "attach");
        self.sinks.lock().push(sink);
        self.mirror.lock().snapshot()
    }

    pub fn frames(&self) -> u64 {
        self.frames.load(Ordering::Relaxed)
    }
}
`

type Op = { origin: DiffLineView['origin']; content: string; old: number | null; new: number | null }

/**
 * Hunks between two texts, at git's three lines of context — what `git_diff_file` and
 * `git_diff_revision` would carry. A plain LCS: the demo's files are a few hundred lines, and
 * computing the hunks rather than writing them out keeps every line number honest against the
 * `oldText`/`newText` the pane rebuilds the whole file from.
 */
export function hunks(before: string, after: string, context = 3): DiffHunkView[] {
  const a = before.split('\n')
  const b = after.split('\n')
  if (a.at(-1) === '') a.pop()
  if (b.at(-1) === '') b.pop()
  const n = a.length
  const m = b.length
  const lcs: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0))
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i]![j] = a[i] === b[j] ? lcs[i + 1]![j + 1]! + 1 : Math.max(lcs[i + 1]![j]!, lcs[i]![j + 1]!)
    }
  }
  const ops: Op[] = []
  let i = 0
  let j = 0
  while (i < n || j < m) {
    if (i < n && j < m && a[i] === b[j]) {
      ops.push({ origin: 'context', content: a[i]!, old: i + 1, new: j + 1 })
      i++
      j++
    } else if (j < m && (i >= n || lcs[i]![j + 1]! >= lcs[i + 1]![j]!)) {
      ops.push({ origin: 'addition', content: b[j]!, old: null, new: j + 1 })
      j++
    } else {
      ops.push({ origin: 'deletion', content: a[i]!, old: i + 1, new: null })
      i++
    }
  }
  // Group changed runs with their context, merging groups whose context would touch.
  const out: DiffHunkView[] = []
  let k = 0
  while (k < ops.length) {
    if (ops[k]!.origin === 'context') {
      k++
      continue
    }
    const start = Math.max(0, k - context)
    let end = k
    for (;;) {
      while (end < ops.length && ops[end]!.origin !== 'context') end++
      let gap = end
      while (gap < ops.length && ops[gap]!.origin === 'context') gap++
      if (gap < ops.length && gap - end <= context * 2) end = gap
      else {
        end = Math.min(ops.length, end + context)
        break
      }
    }
    const slice = ops.slice(start, end)
    const firstOld = slice.find((o) => o.old !== null)?.old ?? 0
    const firstNew = slice.find((o) => o.new !== null)?.new ?? 0
    const oldLines = slice.filter((o) => o.origin !== 'addition').length
    const newLines = slice.filter((o) => o.origin !== 'deletion').length
    // git's section heading: the nearest `fn`/`impl`/`struct` line above the hunk.
    const heading = a
      .slice(0, Math.max(0, firstOld - 1))
      .reverse()
      .find((l) => /^\s*(pub\s+)?(fn|impl|struct|enum|mod)\b/.test(l))
      ?.trim()
    out.push({
      index: out.length,
      header: `@@ -${firstOld},${oldLines} +${firstNew},${newLines} @@${heading ? ` ${heading}` : ''}`,
      oldStart: firstOld,
      oldLines,
      newStart: firstNew,
      newLines,
      lines: slice.map((o) => ({ origin: o.origin, content: o.content, oldLineno: o.old, newLineno: o.new, noNewline: false })),
    })
    k = end
  }
  return out
}
