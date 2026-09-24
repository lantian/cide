/**
 * The GitLab scene's data: merge request !214 on a self-hosted GitLab, the branch the rest of
 * the demo is about (`pty-backpressure`, `data/git.ts`) put up for review.
 *
 * GitLab's own REST shapes (`gitlab/types.ts`) for what `gitlab_request` passes through, and the
 * generated wire types for what cide adds (`GitLabBoard`, `GitLabDraft`, `RevisionDiff`). The
 * diff is *computed* from two texts of `session.rs` rather than written by hand, so the unified
 * diff GitLab would send, the hunks cide draws and the line numbers the threads anchor to cannot
 * disagree — a thread anchored one line off lands under the wrong code, silently.
 */
import type { DiffHunkView, DiffLineView, GitLabBoard, GitLabDraft, RevisionDiff } from '../../ipc/generated'
import type { Approval, Change, Commit, Discussion, MR, Note, Position, User, Version } from '../../gitlab/types'

export const REVIEW = 'rv-00000000-0214'
const ACCOUNT = 'acct-dev'
const HOST = 'https://gitlab.example.com'
const PROJECT = 38
const IID = 214
const MR_URL = `${HOST}/tools/cide/-/merge_requests/${IID}`

// Forty hex characters each — `loadReview` refuses anything else as "still preparing".
const BASE = 'f921463c0be5d7a2e4f18c3b96d0a7e5412c8b9d'
const START = BASE
const HEAD = 'a41c9e07d2b6f5e8c3a1b0d9e8f7a6b5c4d3e2f1'
const refs = { base_sha: BASE, start_sha: START, head_sha: HEAD }

const user = (id: number, username: string, name: string): User => ({
  id,
  username,
  name,
  web_url: `${HOST}/${username}`,
})
const DEV = user(11, 'dev', 'dev')
const MIRA = user(12, 'mira', 'Mira K.')
const ALEX = user(13, 'alex', 'Alex R.')

export const BOARD: GitLabBoard = {
  revision: 1,
  accounts: [{ id: ACCOUNT, host: HOST, username: DEV.username, userId: DEV.id }],
  reviews: [
    {
      id: REVIEW,
      account: ACCOUNT,
      project: PROJECT,
      iid: IID,
      title: 'Split the PTY coalescer and add output backpressure',
      url: MR_URL,
    },
  ],
  preferences: {
    excludeEnabled: true,
    excludedFiles: ['**/*.pb.go', '**/*_grpc.pb.go'],
    reviewPrompt: '',
    reviewPrompts: [],
  },
}

// --- the file under review ------------------------------------------------------------

export const SESSION_PATH = 'crates/cide-pty/src/session.rs'

const SESSION_BASE = `//! One PTY child and the bytes it writes, fanned out to every pane attached to it.

use std::io::{self, Read};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tracing::trace;

use crate::vt::Mirror;
use crate::{Geometry, SessionId, Sink};

/// Flush at least this often, whatever has accumulated.
const FLUSH_EVERY: Duration = Duration::from_millis(16);

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
            pending: Mutex::new(Vec::with_capacity(64 * 1024)),
            last_flush: Mutex::new(Instant::now()),
        }
    }

    /// Feed the reader's bytes in, and flush when the interval has passed.
    pub fn push(&self, bytes: &[u8]) {
        self.mirror.lock().advance(bytes);
        let mut pending = self.pending.lock();
        pending.extend_from_slice(bytes);
        let mut last = self.last_flush.lock();
        if last.elapsed() >= FLUSH_EVERY {
            let frame = std::mem::take(&mut *pending);
            *last = Instant::now();
            drop((pending, last));
            self.flush(frame);
        }
    }

    fn flush(&self, frame: Vec<u8>) {
        trace!(session = %self.id, len = frame.len(), "flush");
        for sink in self.sinks.lock().iter() {
            sink.send(&frame);
        }
    }
}

/// The reader thread: blocking reads until the child closes its end.
pub(crate) fn read_loop(session: Arc<Session>, mut pty: impl Read) -> io::Result<()> {
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = pty.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        session.push(&buf[..n]);
    }
}
`

const SESSION_HEAD = `//! One PTY child and the bytes it writes, fanned out to every pane attached to it.

use std::io::{self, Read};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Condvar, Mutex};
use tracing::trace;

use crate::coalesce::{Coalescer, Pressure};
use crate::vt::Mirror;
use crate::{Geometry, SessionId, Sink};

/// How long a held reader waits for an ack before it re-checks the window.
const HOLD_POLL: Duration = Duration::from_millis(50);

pub struct Session {
    id: SessionId,
    mirror: Mutex<Mirror>,
    sinks: Mutex<Vec<Arc<dyn Sink>>>,
    /// The frame under construction and the ack window; see \`coalesce.rs\`.
    coalescer: Mutex<Coalescer>,
    /// Signalled by \`ack\`, so a held reader wakes the moment the window reopens.
    acked: Condvar,
}

impl Session {
    pub fn new(id: SessionId, geometry: Geometry) -> Self {
        Self {
            id,
            mirror: Mutex::new(Mirror::new(geometry)),
            sinks: Mutex::new(Vec::new()),
            coalescer: Mutex::new(Coalescer::default()),
            acked: Condvar::new(),
        }
    }

    /// Feed the reader's bytes in. \`Pressure::Hold\` tells the reader to stop reading until
    /// the webview acknowledges what it already has.
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

    /// The webview has painted \`len\` bytes; reopen the window by that much.
    pub fn ack(&self, len: usize) {
        self.coalescer.lock().ack(len);
        self.acked.notify_all();
    }
}

/// The reader thread: blocking reads until the child closes its end, pausing while held.
pub(crate) fn read_loop(session: Arc<Session>, mut pty: impl Read) -> io::Result<()> {
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = pty.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        while session.push(&buf[..n]) == Pressure::Hold {
            let mut coalescer = session.coalescer.lock();
            session.acked.wait_for(&mut coalescer, HOLD_POLL);
        }
    }
}
`

/** The 1-based line in `text` that contains `needle`; throws, so a reworded line fails loudly. */
function lineOf(text: string, needle: string): number {
  const at = text.split('\n').findIndex((l) => l.includes(needle))
  if (at < 0) throw new Error(`demo gitlab: no line with ${needle}`)
  return at + 1
}

// --- a line diff, so every derived shape agrees -----------------------------------------

interface Row {
  origin: DiffLineView['origin']
  content: string
  old: number | null
  new: number | null
}

/** Longest-common-subsequence line diff. The files are a hundred lines; O(n·m) is nothing. */
function rows(a: string[], b: string[]): Row[] {
  const n = a.length
  const m = b.length
  const lcs: number[][] = Array.from({ length: n + 1 }, () => Array<number>(m + 1).fill(0))
  for (let i = n - 1; i >= 0; i--)
    for (let j = m - 1; j >= 0; j--)
      lcs[i]![j] = a[i] === b[j] ? lcs[i + 1]![j + 1]! + 1 : Math.max(lcs[i + 1]![j]!, lcs[i]![j + 1]!)
  const out: Row[] = []
  let i = 0
  let j = 0
  while (i < n || j < m) {
    if (i < n && j < m && a[i] === b[j]) {
      out.push({ origin: 'context', content: a[i]!, old: i + 1, new: j + 1 })
      i++
      j++
    } else if (i < n && (j >= m || lcs[i + 1]![j]! >= lcs[i]![j + 1]!)) {
      // Deletions first on a tie, as git orders a replaced block.
      out.push({ origin: 'deletion', content: a[i]!, old: i + 1, new: null })
      i++
    } else {
      out.push({ origin: 'addition', content: b[j]!, old: null, new: j + 1 })
      j++
    }
  }
  return out
}

/** Group rows into hunks with three lines of context, as `git diff` would. */
function hunks(all: Row[]): DiffHunkView[] {
  const CONTEXT = 3
  const changed = all.map((r, k) => (r.origin === 'context' ? -1 : k)).filter((k) => k >= 0)
  const spans: Array<[number, number]> = []
  for (const k of changed) {
    const from = Math.max(0, k - CONTEXT)
    const to = Math.min(all.length - 1, k + CONTEXT)
    const last = spans.at(-1)
    if (last && from <= last[1] + 1) last[1] = to
    else spans.push([from, to])
  }
  return spans.map(([from, to], index) => {
    const lines = all.slice(from, to + 1)
    const oldStart = lines.find((l) => l.old !== null)?.old ?? 0
    const newStart = lines.find((l) => l.new !== null)?.new ?? 0
    const oldLines = lines.filter((l) => l.origin !== 'addition').length
    const newLines = lines.filter((l) => l.origin !== 'deletion').length
    return {
      index,
      header: `@@ -${oldStart},${oldLines} +${newStart},${newLines} @@`,
      oldStart,
      oldLines,
      newStart,
      newLines,
      lines: lines.map((l) => ({ origin: l.origin, content: l.content, oldLineno: l.old, newLineno: l.new, noNewline: false })),
    }
  })
}

function unified(h: DiffHunkView[]): string {
  const sign = { context: ' ', addition: '+', deletion: '-' } as const
  return h.map((x) => [x.header, ...x.lines.map((l) => sign[l.origin] + l.content)].join('\n')).join('\n') + '\n'
}

const split = (t: string) => (t === '' ? [] : t.replace(/\n$/, '').split('\n'))

function fileChange(path: string, before: string, after: string): { change: Change; diff: RevisionDiff } {
  const h = hunks(rows(split(before), split(after)))
  const added = before === ''
  return {
    change: {
      old_path: path,
      new_path: path,
      diff: unified(h),
      new_file: added,
      deleted_file: false,
      renamed_file: false,
    },
    diff: {
      path,
      oldPath: null,
      new: { kind: 'commit', oid: HEAD },
      old: { kind: 'commit', oid: BASE },
      newOid: '5d0c2f7a9e1b3c4d5e6f7a8b9c0d1e2f3a4b5c6d',
      oldOid: added ? null : '8e4b1a2c3d5f6e7a8b9c0d1e2f3a4b5c6d7e8f90',
      status: added ? 'added' : 'modified',
      binary: false,
      oldMode: 0o100644,
      newMode: 0o100644,
      hunks: h,
      oldText: before,
      newText: after,
      textsOmitted: false,
    },
  }
}

// --- the other files in the MR ----------------------------------------------------------

const COALESCE = `//! Frames, and when a session may send the next one.

/// What \`push\` tells the reader: go on, or stop until the webview catches up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressure {
    Flow,
    Hold,
}

/// Bytes the webview may have in flight before the reader is told to hold.
const WINDOW: usize = 8 * 64 * 1024;

#[derive(Debug, Default)]
pub struct Coalescer {
    pending: Vec<u8>,
    unacked: usize,
}

impl Coalescer {
    pub fn push(&mut self, bytes: &[u8]) -> Pressure {
        self.pending.extend_from_slice(bytes);
        if self.unacked + self.pending.len() > WINDOW {
            return Pressure::Hold;
        }
        Pressure::Flow
    }

    pub fn take_due(&mut self) -> Option<Vec<u8>> {
        let cut = crate::vt::frame_boundary(&self.pending)?;
        let frame: Vec<u8> = self.pending.drain(..cut).collect();
        self.unacked += frame.len();
        Some(frame)
    }

    pub fn ack(&mut self, len: usize) {
        self.unacked = self.unacked.saturating_sub(len);
    }
}
`

const LIB_BASE = `pub mod session;
pub mod vt;

pub use session::Session;
`
const LIB_HEAD = `pub mod coalesce;
pub mod session;
pub mod vt;

pub use coalesce::Pressure;
pub use session::Session;
`

const TEST = `use cide_pty::testing::{flood, slow_sink};

#[test]
fn a_flood_holds_the_reader_instead_of_buffering() {
    let (session, sink) = slow_sink();
    let peak = flood(&session, 200 * 1024 * 1024);
    assert!(peak <= 8 * 64 * 1024, "buffered {peak} bytes past the window");
    assert_eq!(sink.received(), 200 * 1024 * 1024);
}
`

const JOURNAL_BASE = `## M102 — small freezes, found by reading

Every freeze in this batch was a lock held across a call that could block.
`
const JOURNAL_HEAD = `## M102 — small freezes, found by reading

Every freeze in this batch was a lock held across a call that could block.

## M103 — backpressure

The coalescer is its own module, and a reader that gets ahead of the webview now waits for an
ack instead of buffering. \`yes | head -c 200M\` peaks at 512 KiB in flight, down from 200 MiB.
`

const FILES = [
  fileChange('crates/cide-pty/src/coalesce.rs', '', COALESCE),
  fileChange(SESSION_PATH, SESSION_BASE, SESSION_HEAD),
  fileChange('crates/cide-pty/src/lib.rs', LIB_BASE, LIB_HEAD),
  fileChange('crates/cide-pty/tests/backpressure.rs', '', TEST),
  fileChange('docs/journal.md', JOURNAL_BASE, JOURNAL_HEAD),
]

/** `comparison` answers, keyed by path. */
export const COMPARISONS = new Map(FILES.map((f) => [f.diff.path, f.diff]))
/** `file` answers, keyed by `sha:path`. */
export const BLOBS = new Map(
  FILES.flatMap((f) => [
    [`${BASE}:${f.diff.path}`, f.diff.oldText ?? ''],
    [`${HEAD}:${f.diff.path}`, f.diff.newText ?? ''],
  ]),
)

export const MERGE_REQUEST: MR = {
  id: 90214,
  iid: IID,
  project_id: PROJECT,
  title: 'Split the PTY coalescer and add output backpressure',
  description: `Moves coalescing out of \`session.rs\` into \`coalesce.rs\`, and makes the reader **wait** when the webview is more than a window behind instead of buffering without bound.

- \`Session::push\` now answers with a \`Pressure\`
- \`read_loop\` parks on a condvar until \`ack\` reopens the window
- New test: a 200 MiB flood never has more than 512 KiB in flight

Closes #188.`,
  web_url: MR_URL,
  state: 'opened',
  draft: false,
  source_branch: 'pty-backpressure',
  target_branch: 'master',
  source_project_id: PROJECT,
  target_project_id: PROJECT,
  author: MIRA,
  reviewers: [DEV, ALEX],
  assignees: [MIRA],
  diff_refs: refs,
  sha: HEAD,
  updated_at: '2026-09-24T13:52:00.000Z',
  references: { full: `tools/cide!${IID}` },
  user_notes_count: 6,
  source_project: { web_url: `${HOST}/tools/cide` },
}

const VERSION_BASE = {
  ...refs,
  id: 5021,
  base_commit_sha: BASE,
  start_commit_sha: START,
  head_commit_sha: HEAD,
  state: 'collected',
  real_size: String(FILES.length),
}

export const VERSIONS: Version[] = [VERSION_BASE]
export const VERSION: Version = { ...VERSION_BASE, diffs: FILES.map((f) => f.change) }

// --- the conversation -------------------------------------------------------------------

const position = (path: string, line: number): Position => ({
  ...refs,
  position_type: 'text',
  old_path: path,
  new_path: path,
  new_line: line,
})

let noteId = 70400
const note = (author: User, body: string, created: string, extra: Partial<Note> = {}): Note => ({
  id: noteId++,
  body,
  author,
  created_at: created,
  system: false,
  ...extra,
})

const HOLD_LINE = lineOf(SESSION_HEAD, 'session.acked.wait_for')
const POLL_LINE = lineOf(SESSION_HEAD, 'const HOLD_POLL')
export const NOTIFY_LINE = lineOf(SESSION_HEAD, 'self.acked.notify_all()')

export const DISCUSSIONS: Discussion[] = [
  {
    id: 'd-hold',
    individual_note: false,
    notes: [
      note(
        ALEX,
        'What wakes this if the pane is **detached** while the reader is held? No sink will ever ack, so the child blocks on its write forever.',
        '2026-09-24T12:41:00.000Z',
        { resolvable: true, resolved: false, position: position(SESSION_PATH, HOLD_LINE) },
      ),
      note(
        MIRA,
        'Good catch — `detach` should drop the unacked count for that sink. Adding it plus a test that detaches mid-flood.',
        '2026-09-24T13:05:00.000Z',
        { resolvable: true, resolved: false, position: position(SESSION_PATH, HOLD_LINE) },
      ),
    ],
  },
  {
    id: 'd-poll',
    individual_note: false,
    notes: [
      note(ALEX, 'nit: why 50 ms? A frame is 16.', '2026-09-24T12:30:00.000Z', {
        resolvable: true,
        resolved: true,
        position: position(SESSION_PATH, POLL_LINE),
      }),
      note(MIRA, 'It only bounds a missed notify; the condvar wakes it first. Comment added.', '2026-09-24T12:58:00.000Z', {
        resolvable: true,
        resolved: true,
        position: position(SESSION_PATH, POLL_LINE),
      }),
    ],
  },
  {
    id: 'd-general',
    individual_note: true,
    notes: [note(ALEX, 'Ran the flood test locally: 512 KiB peak, down from 200 MiB. Nice.', '2026-09-24T12:20:00.000Z')],
  },
]

export const ACTIVITY: Note[] = [
  note(MIRA, 'requested review from @dev and @alex', '2026-09-24T10:02:00.000Z', { system: true }),
  note(MIRA, 'added 2 commits', '2026-09-24T11:48:00.000Z', { system: true }),
  ...DISCUSSIONS.flatMap((d) => d.notes),
  note(ALEX, 'approved this merge request', '2026-09-24T13:10:00.000Z', { system: true }),
].sort((a, b) => a.created_at.localeCompare(b.created_at))

export const APPROVAL: Approval = {
  approved: false,
  approved_by: [{ user: ALEX }],
  approvals_required: 2,
  approvals_left: 1,
  user_can_approve: true,
}

export const COMMITS: Commit[] = [
  ['a41c9e07', 'Wake a held reader on ack, not on a timer'],
  ['7be3d215', 'Hold frames past the ack window'],
  ['c90f14a8', 'Test a 200 MiB flood against the window'],
  ['3d6a8e52', 'Split the coalescer out of session.rs'],
].map(([short, title], k) => ({
  id: `${short}${'0'.repeat(32)}`,
  short_id: short!,
  title: title!,
  author_name: MIRA.name,
  authored_date: `2026-09-24T1${1 - Math.min(k, 1)}:${String(40 - k * 9).padStart(2, '0')}:00Z`,
  web_url: `${HOST}/tools/cide/-/commit/${short}`,
}))

/** The agent's unpublished finding: drawn dashed, under the line it is about. */
export const DRAFTS: GitLabDraft[] = [
  {
    id: 'draft-notify',
    review: REVIEW,
    severity: 'major',
    body: '`notify_all` runs after the coalescer lock is released, so an ack that lands between `push` returning `Hold` and `wait_for` taking the lock is missed — the reader then sleeps a full `HOLD_POLL`. Notify while holding the lock, or re-check `unacked` before waiting.',
    path: SESSION_PATH,
    oldPath: SESSION_PATH,
    side: 'new',
    line: NOTIFY_LINE,
    position: position(SESSION_PATH, NOTIFY_LINE),
    headSha: HEAD,
    author: { label: `Review !${IID}`, harness: 'claude', run: 'run-review-214', conversation: null },
    createdUnixMs: 1_790_000_000_000,
    replies: [],
  },
]

export const DOCUMENT = {
  review: REVIEW,
  path: SESSION_PATH,
  oldPath: SESSION_PATH,
  baseSha: BASE,
  startSha: START,
  headSha: HEAD,
  mode: 'diff' as const,
  newFile: false,
  deletedFile: false,
}
