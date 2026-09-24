/**
 * The editor scene's document: `crates/cide-pty/src/session.rs` mid-edit, on the branch that
 * splits the coalescer out (`data/git.ts`), with the caret halfway through an identifier so a
 * Ctrl+Space has something to complete.
 *
 * Written as the file would read after the split rather than lifted from `lib.rs`: the real
 * crate still keeps the coalescer inline, and the story the other panes tell is the change that
 * moves it. `HEAD` is the same file minus the edit, so the change column has something to draw.
 */
import type {
  CompletionAnswer,
  CompletionItem,
  Diagnostic,
  FileDoc,
  FileOutline,
  RepoPath,
  RevisionBlob,
  Symbol as Sym,
  ViewPosition,
} from '../../ipc/generated'
import type { Handler } from '../fakeTauri'
import { HOME, wire } from '../world'
import { REPO } from './git'

export const SESSION_PATH = `${HOME}/crates/cide-pty/src/session.rs`

/** Where the caret sits: after the partial identifier on the line that carries it. */
const CARET_MARK = '⸢caret⸣'

const WORKING = String.raw`//! One PTY-backed session: the child, its reader thread, and the sinks watching it.
//!
//! The coalescer that used to live here is in [` + '`crate::coalesce`' + String.raw`] now. What stays is the
//! bookkeeping a session owes each sink — how much it has been sent, how much it has acked,
//! and whether it has been skipped long enough to need a catch-up frame instead of bytes.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, Sender, bounded};
use parking_lot::Mutex;

use crate::coalesce::{Coalescer, Frame};
use crate::{CreditPolicy, Geometry, PtyError, Sink, SinkId};

/// Depth of the reader→coalescer channel. Small on purpose: this is the backpressure.
const READ_QUEUE_DEPTH: usize = 16;

/// What one attached webview owes, and whether it is still being sent raw output.
#[derive(Debug)]
pub(crate) struct Registered {
    pub id: SinkId,
    pub sink: Arc<dyn Sink>,
    /// Bytes sent and not yet acknowledged.
    pub outstanding: AtomicUsize,
    /// Set while the sink is above [` + '`CreditPolicy::high`' + String.raw`]; cleared below ` + '`low`' + String.raw`.
    pub choked: AtomicBool,
    /// When the sink last acked, for the watchdog that force-resets a wedged one.
    pub last_ack: Mutex<Instant>,
}

pub struct PtySession {
    geometry: Mutex<Geometry>,
    policy: CreditPolicy,
    sinks: Mutex<Vec<Arc<Registered>>>,
    frames: Sender<Frame>,
    exited: AtomicBool,
}

impl PtySession {
    pub(crate) fn start(policy: CreditPolicy, geometry: Geometry) -> (Arc<Self>, Receiver<Frame>) {
        let (frames, rx) = bounded(READ_QUEUE_DEPTH);
        let session = Arc::new(Self {
            geometry: Mutex::new(geometry),
            policy,
            sinks: Mutex::new(Vec::new()),
            frames,
            exited: AtomicBool::new(false),
        });
        (session, rx)
    }

    /// Record an acknowledgement from the webview, and un-choke the sink once it has caught up.
    pub fn ack(&self, id: SinkId, bytes: usize) {
        let Some(sink) = self.find(id) else { return };
        let before = sink.outstanding.fetch_sub(bytes.min(sink.outstanding.load(Ordering::Relaxed)), Ordering::AcqRel);
        *sink.last_ack.lock() = Instant::now();
        if before.saturating_sub(bytes) <= self.policy.low {
            sink.choked.store(false, Ordering::Release);
        }
    }

    /// Deliver one coalesced frame to every sink that still has credit.
    ///
    /// A choked sink is skipped, never waited on: blocking here would freeze the agent's
    /// terminal because one occluded window stopped acking. It owes a catch-up frame instead,
    /// and the vt100 mirror has everything it missed.
    pub(crate) fn broadcast(&self, frame: &Frame, now: Instant) -> usize {
        let mut delivered = 0;
        for sink in self.sinks.lock().iter() {
            if sink.choked.load(Ordering::Acquire) {
                let idle = now.duration_since(*sink.last_ack.lock());
                if idle < self.policy.watchdog {
                    continue;
                }
                tracing::warn!(sink = %sink.id, ?idle, "sink stopped acking; resetting its credit");
                sink.outstanding.store(0, Ordering::Release);
                sink.choked.store(false, Ordering::Release);
            }
            let owed = sink.outstanding.fetch_add(frame.len(), Ordering::AcqRel) + frame.len();
            if owed > self.policy.high {
                sink.choked.store(true, Ordering::Release);
            }
            if sink.sink.deliver(frame.bytes()) {
                delivered += 1;
            }
        }
        delivered
    }

    /// Hand a frame to the coalescer, holding it back while the queue is full.
    pub(crate) fn submit(&self, frame: Frame) -> Result<(), PtyError> {
        let window = Duration::from_millis(250);
        let credit = Credit${CARET_MARK}
        self.frames
            .send_timeout(frame, window)
            .map_err(|_| PtyError::Backpressure { depth: READ_QUEUE_DEPTH })
    }

    fn find(&self, id: SinkId) -> Option<Arc<Registered>> {
        self.sinks.lock().iter().find(|s| s.id == id).cloned()
    }

    pub fn resize(&self, geometry: Geometry) -> Result<(), PtyError> {
        *self.geometry.lock() = geometry;
        Ok(())
    }

    pub fn has_exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_choked_sink_is_skipped_not_waited_on() {
        let (session, _rx) = PtySession::start(CreditPolicy::default(), Geometry::new(80, 24, 8, 16));
        assert!(!session.has_exited());
    }
}
`

const lines = WORKING.split('\n')
const caretLine = lines.findIndex((l) => l.includes(CARET_MARK))

/** The file as the editor reads it, with the marker taken out. */
export const SESSION_TEXT = WORKING.replace(CARET_MARK, '')

/** The file with the half-typed line gone, for a scene that is not about typing. */
export const SESSION_CLEAN = SESSION_TEXT.split('\n').filter((_, i) => i !== caretLine).join('\n')

/** 1-based, as `ViewPosition` counts. */
export const CARET_LINE = caretLine + 1
export const CARET_COLUMN = (lines[caretLine] ?? '').indexOf(CARET_MARK) + 1

/**
 * `HEAD`'s copy, for the change column: before this branch the session called the coalescer
 * inline and had no `submit`, and the watchdog only logged. Three shapes of change — added
 * lines, a modified run, a deletion — so all three bar styles are on screen.
 */
export const SESSION_HEAD = SESSION_TEXT
  .replace('use crate::coalesce::{Coalescer, Frame};\n', 'use crate::coalesce_inline::Frame;\n')
  .replace(
    /    \/\/\/ Hand a frame to the coalescer[\s\S]*?\n    }\n\n/,
    '',
  )
  .replace(
    '                sink.outstanding.store(0, Ordering::Release);\n                sink.choked.store(false, Ordering::Release);\n',
    '                sink.choked.store(false, Ordering::Release);\n',
  )
  .replace(
    '    pub fn has_exited(&self) -> bool {',
    '    /// Whether the child has gone; the reader thread sets it on EOF.\n    pub fn has_exited(&self) -> bool {',
  )

export const SESSION_POSITION: ViewPosition = {
  path: SESSION_PATH,
  topLine: Math.max(1, CARET_LINE - 18),
  line: CARET_LINE,
  column: CARET_COLUMN,
  folds: [],
  markdownView: 'text',
  touchedAt: wire(0),
}

function item(
  label: string,
  kind: CompletionItem['kind'],
  detail: string | null,
  description: string | null,
  sort: number,
  extraEdits: CompletionItem['extraEdits'] = [],
): CompletionItem {
  return {
    label,
    filterText: label,
    detail,
    description,
    kind,
    insert: label,
    snippet: false,
    replace: null,
    sort,
    extraEdits,
    resolve: null,
    deprecated: false,
  }
}

/** The line the `use crate::coalesce::…` import sits on, where an auto-import lands after. */
const importLine = lines.findIndex((l) => l.startsWith('use crate::{')) + 1

/**
 * What rust-analyzer offers after `Credit`: the in-scope names first, then a type from another
 * module that accepting would import — the auto-import row is the one worth a picture.
 */
export const COMPLETION: CompletionAnswer = {
  kind: 'items',
  token: 1,
  incomplete: false,
  truncated: false,
  items: [
    item('CreditWindow', 'struct', ' (use crate::coalesce::CreditWindow)', 'crate::coalesce', 0, [
      { line: importLine + 1, column: 1, endLine: importLine + 1, endColumn: 1, text: 'use crate::coalesce::CreditWindow;\n' },
    ]),
    item('CreditWindow::new(…)', 'function', '(policy: CreditPolicy, now: Instant)', 'fn(CreditPolicy, Instant) -> CreditWindow', 1, [
      { line: importLine + 1, column: 1, endLine: importLine + 1, endColumn: 1, text: 'use crate::coalesce::CreditWindow;\n' },
    ]),
    item('CreditWatchdog', 'struct', ' (use crate::coalesce::CreditWatchdog)', 'crate::coalesce', 2, [
      { line: importLine + 1, column: 1, endLine: importLine + 1, endColumn: 1, text: 'use crate::coalesce::CreditWatchdog;\n' },
    ]),
    item('CreditPolicy', 'struct', null, 'crate::CreditPolicy', 3),
    item('CreditPolicy::default()', 'function', '()', 'fn() -> CreditPolicy', 4),
    item('CreditLedger', 'struct', ' (use crate::ledger::CreditLedger)', 'crate::ledger', 5, [
      { line: importLine + 1, column: 1, endLine: importLine + 1, endColumn: 1, text: 'use crate::ledger::CreditLedger;\n' },
    ]),
    item('ControlFlow', 'enum', ' (use std::ops::ControlFlow)', 'std::ops', 5, [
      { line: importLine + 1, column: 1, endLine: importLine + 1, endColumn: 1, text: 'use std::ops::ControlFlow;\n' },
    ]),
    item('Cow', 'enum', ' (use std::borrow::Cow)', 'std::borrow', 6),
    item('crate', 'keyword', null, null, 7),
  ],
}

/** One warning and one error, both where the edit is, so the lint underline is on screen. */
export function sessionDiagnostics(): Diagnostic[] {
  const at = (needle: string) => lines.findIndex((l) => l.includes(needle)) + 1
  const coalescer = at('use crate::coalesce::{Coalescer, Frame};')
  const credit = CARET_LINE
  return [
    {
      path: 'crates/cide-pty/src/session.rs',
      absPath: SESSION_PATH,
      line: coalescer,
      column: 25,
      endLine: coalescer,
      endColumn: 34,
      severity: 'warning',
      kind: 'semantic',
      message: 'unused import: `Coalescer`',
      source: 'rust-analyzer',
      code: 'unused_imports',
      stale: false,
    },
    {
      path: 'crates/cide-pty/src/session.rs',
      absPath: SESSION_PATH,
      line: credit,
      column: 22,
      endLine: credit,
      endColumn: 28,
      severity: 'error',
      kind: 'semantic',
      message: 'cannot find value `Credit` in this scope',
      source: 'rust-analyzer',
      code: 'E0425',
      stale: false,
    },
  ]
}

/**
 * The outline rust-analyzer would give, read off the text by indentation — top-level items, and
 * the methods of each `impl`/`mod` as its children. Enough for a file this regular; the point is
 * that `symbols_outline` answers `ready` rather than the empty value, which the pane cannot take.
 */
export function sessionOutline(): FileOutline {
  const top: Sym[] = []
  let parent: Sym | null = null
  lines.forEach((text, i) => {
    const m = /^( *)(?:pub(?:\(crate\))? )?(struct|fn|impl|const|mod) (\w+)/.exec(text)
    if (!m) return
    const indent = (m[1] ?? '').length
    const kind: Sym['kind'] =
      m[2] === 'fn' ? (indent > 0 ? 'method' : 'function')
        : m[2] === 'impl' ? 'impl'
          : m[2] === 'const' ? 'constant'
            : m[2] === 'mod' ? 'module'
              : 'struct'
    const name = m[3] ?? ''
    const col = text.indexOf(name) + 1
    const span = { startLine: i + 1, startColumn: indent + 1, endLine: i + 1, endColumn: text.length + 1 }
    const sym: Sym = { kind, name, detail: null, container: null, range: span, selection: { ...span, startColumn: col, endColumn: col + name.length }, children: [] }
    if (indent === 0) {
      top.push(sym)
      parent = kind === 'impl' || kind === 'module' ? sym : null
    } else if (parent) {
      parent.children.push({ ...sym, container: parent.name })
    }
  })
  return { kind: 'ready', language: 'rust', symbols: top, truncated: false }
}

/**
 * What an editor pane on `session.rs` asks for before it will draw: the text, where the view was,
 * HEAD's copy for the change column, and the outline (whose empty value the pane cannot take).
 * Shared by every scene that opens the file, so none of them draws it half-loaded.
 */
export function sessionFileHandlers(position: ViewPosition | null, text = SESSION_TEXT): Array<[string, Handler]> {
  const rel = (p: string) => p.slice(HOME.length + 1)
  return [
    ['file_read', (a): FileDoc => ({
      path: String(a['path']),
      text: String(a['path']) === SESSION_PATH ? text : '',
      writable: true,
      stamp: { mtimeNanos: '1790000000000000000', len: text.length },
    })],
    ['file_position', (a): ViewPosition | null => (String(a['path']) === SESSION_PATH ? position : null)],
    ['git_locate', (a): RepoPath | null => ({ repo: REPO.id, path: rel(String(a['path'])) })],
    ['git_file_at_revision', (): RevisionBlob => ({
      text: SESSION_HEAD,
      oid: '5c1e0d9a7b3f2e8d4c6a1b0f9e8d7c6b5a4f3e2d',
      binary: false,
      bytes: SESSION_HEAD.length,
      truncated: false,
    })],
    ['symbols_outline', (): FileOutline => sessionOutline()],
  ]
}
