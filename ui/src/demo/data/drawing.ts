/**
 * The drawing scene's two documents: `docs/pty-backpressure.md`, the design note for the branch
 * that splits the coalescer out (`data/git.ts`), and `docs/pty-backpressure.excalidraw`, the
 * diagram the note is about.
 *
 * The drawing is a real Excalidraw 0.18 scene, built here rather than pasted as a blob, so that
 * the geometry is arithmetic someone can move a box in: every arrow's end is computed from the
 * box it binds to, and every bound label is centred on its container. Excalidraw's `restore`
 * fills the fields a file may omit, but not the bindings — an arrow whose `startBinding` names a
 * box whose `boundElements` does not name it back comes loose the first time either is dragged,
 * so both sides are written.
 */
import type { FileBytesHead, FileDoc, FileOutline, TreeRow, TreeStatusMap, ViewPosition } from '../../ipc/generated'
import type { Handler } from '../fakeTauri'
import { HOME, TREE_STATUS, treeRows, wire } from '../world'

export const NOTE_PATH = `${HOME}/docs/pty-backpressure.md`
export const DRAWING_PATH = `${HOME}/docs/pty-backpressure.excalidraw`

export const NOTE_TEXT = `# PTY backpressure

A \`claude\` session can print faster than a webview can paint. Until this branch the reader
thread pushed every read straight into the channel, and one occluded window that stopped
acking would grow the queue without bound. The coalescer now lives in its own module
(\`crates/cide-pty/src/coalesce.rs\`) and owns the credit window.

See the diagram: [pty-backpressure.excalidraw](pty-backpressure.excalidraw).

## The path of a byte

1. The **reader thread** reads the PTY in 64 KiB chunks.
2. The **coalescer** folds them into frames — one per 16 ms, or 256 KiB, whichever first.
3. The frame goes over the Tauri \`Channel\`; a small one rides \`webview.eval\` instead.
4. **xterm.js** writes it and acks the byte count once it has parsed it.

## Credit

| knob | default | meaning |
| --- | --- | --- |
| \`high\` | 2 MiB | unacked bytes that choke a sink |
| \`low\` | 512 KiB | where a choked sink is let back in |
| \`watchdog\` | 5 s | a sink silent this long is reset, not waited on |

\`\`\`rust
if owed > self.policy.high {
    sink.choked.store(true, Ordering::Release);
}
\`\`\`

> A choked sink is **skipped**, never waited on — the vt100 mirror holds what it missed, and
> it gets one catch-up frame when it acks again.
`

/** Open on the rendered side: the note is the kind of file one reads rather than edits. */
export const NOTE_POSITION: ViewPosition = {
  path: NOTE_PATH,
  topLine: 1,
  line: 1,
  column: 1,
  folds: [],
  markdownView: 'preview',
  touchedAt: wire(0),
}

/* ------------------------------------------------------------------------------ the drawing */

type El = Record<string, unknown> & { id: string; boundElements?: Array<{ id: string; type: string }> }

let seed = 1000
const base = (id: string, type: string, x: number, y: number, width: number, height: number): El => ({
  id,
  type,
  x,
  y,
  width,
  height,
  angle: 0,
  strokeColor: '#1e1e1e',
  backgroundColor: 'transparent',
  fillStyle: 'solid',
  strokeWidth: 2,
  strokeStyle: 'solid',
  roughness: 1,
  opacity: 100,
  groupIds: [],
  frameId: null,
  roundness: null,
  seed: ++seed,
  version: 1,
  versionNonce: seed * 7,
  isDeleted: false,
  boundElements: [],
  updated: 1790000000000,
  link: null,
  locked: false,
})

/** A rough width for Excalifont at `size` — close enough for a centred label to look centred. */
const measure = (line: string, size: number) => Math.round(line.length * size * 0.52)

function text(id: string, x: number, y: number, body: string, size: number, opts: Partial<El> = {}): El {
  const lines = body.split('\n')
  const width = Math.max(...lines.map((l) => measure(l, size)))
  const height = Math.round(lines.length * size * 1.25)
  return {
    ...base(id, 'text', x, y, width, height),
    text: body,
    originalText: body,
    fontSize: size,
    fontFamily: 5,
    textAlign: 'left',
    verticalAlign: 'top',
    containerId: null,
    autoResize: true,
    lineHeight: 1.25,
    ...opts,
  }
}

interface Box {
  el: El
  label: El
}

/** A filled, rounded box with its label bound inside it and centred. */
function box(id: string, x: number, y: number, w: number, h: number, fill: string, label: string, shape = 'rectangle'): Box {
  const el: El = { ...base(id, shape, x, y, w, h), backgroundColor: fill, roundness: shape === 'rectangle' ? { type: 3 } : { type: 2 } }
  const size = 22
  const lines = label.split('\n')
  const tw = Math.max(...lines.map((l) => measure(l, size)))
  const th = Math.round(lines.length * size * 1.25)
  const t = text(`${id}-label`, x + (w - tw) / 2, y + (h - th) / 2, label, size, {
    textAlign: 'center',
    verticalAlign: 'middle',
    containerId: id,
  })
  el.boundElements = [{ id: t.id, type: 'text' }]
  return { el, label: t }
}

/**
 * An arrow from one box to another along `points` (relative to the first), bound at both ends.
 * `from`/`to` get the arrow in their `boundElements` too — see the header.
 */
function arrow(id: string, from: Box, to: Box, x: number, y: number, points: Array<[number, number]>, opts: Partial<El> = {}): El {
  const xs = points.map((p) => p[0])
  const ys = points.map((p) => p[1])
  const el: El = {
    ...base(id, 'arrow', x, y, Math.max(...xs) - Math.min(...xs), Math.max(...ys) - Math.min(...ys)),
    roundness: { type: 2 },
    points,
    lastCommittedPoint: null,
    startBinding: { elementId: from.el.id, focus: 0, gap: 6 },
    endBinding: { elementId: to.el.id, focus: 0, gap: 6 },
    startArrowhead: null,
    endArrowhead: 'arrow',
    elbowed: false,
    ...opts,
  }
  from.el.boundElements?.push({ id, type: 'arrow' })
  to.el.boundElements?.push({ id, type: 'arrow' })
  return el
}

/**
 * claude PTY → Coalescer → Channel → xterm.js, with the ack loop running back underneath from
 * the terminal to the coalescer — the one arrow that is the point of the branch, so it is the
 * one drawn dashed and in red.
 */
function drawingElements(): El[] {
  // Sized for the pane at 1440×900 at 100%: `scrollToContent` centres a drawing but does not
  // zoom it, so a scene wider than ~1100 px comes up with its last box cut off.
  const W = 170
  const H = 90
  const GAP = 112
  const Y = 170
  const col = (i: number) => 40 + i * (W + GAP)
  const pty = box('pty', col(0), Y, W, H, '#ffec99', 'claude PTY')
  const coal = box('coalescer', col(1), Y, W, H, '#a5d8ff', 'Coalescer')
  const chan = box('channel', col(2), Y, W, H, '#d0bfff', 'Channel')
  const term = box('xterm', col(3), Y, W, H, '#b2f2bb', 'xterm.js')
  const mid = Y + H / 2
  const hop = (a: Box, b: Box, id: string) =>
    arrow(id, a, b, (a.el.x as number) + W + 6, mid, [[0, 0], [GAP - 12, 0]])
  const a1 = hop(pty, coal, 'a-read')
  const a2 = hop(coal, chan, 'a-frame')
  const a3 = hop(chan, term, 'a-send')
  const hopLabel = (id: string, a: El, body: string) => {
    const w = Math.max(...body.split('\n').map((l) => measure(l, 16)))
    return text(id, (a.x as number) + (GAP - 12) / 2 - w / 2, mid - 60, body, 16, { strokeColor: '#495057' })
  }
  // Out of the terminal's bottom edge, under the channel, up into the coalescer's: the one
  // arrow that is the point of the branch, so it is the one drawn dashed and in red.
  const loopX = col(3) + W / 2
  const loopY = Y + H + 6
  const back = col(1) + W / 2 - loopX
  const ack = arrow('a-ack', term, coal, loopX, loopY, [[0, 0], [-30, 90], [back / 2, 130], [back + 30, 90], [back, 0]], {
    strokeColor: '#e03131',
    strokeStyle: 'dashed',
  })
  const ackText = 'ack window: owed > high → choke\nacked below low → let back in'
  const ackW = measure('ack window: owed > high → choke', 18)
  // Down and to the left, away from the loop, so the two never cross; into the mirror's top so
  // the note beside it keeps its side clear.
  const sink = box('mirror', col(0), Y + H + 190, W, 80, '#ffc9c9', 'vt100 mirror')
  const toMirror = arrow('a-mirror', coal, sink, col(1) + 30, Y + H + 6, [[0, 0], [-50, 110], [col(0) + W / 2 - (col(1) + 30), 178]], {
    strokeStyle: 'dotted',
    strokeColor: '#868e96',
  })
  return [
    text('title', 40, 40, 'PTY → webview, with backpressure', 36),
    text('subtitle', 42, 94, 'crates/cide-pty · coalesce.rs split out of session.rs', 18, { strokeColor: '#868e96' }),
    pty.el, pty.label,
    coal.el, coal.label,
    chan.el, chan.label,
    term.el, term.label,
    a1, a2, a3,
    hopLabel('l-read', a1, '64 KiB\nreads'),
    hopLabel('l-frame', a2, '16 ms or\n256 KiB'),
    hopLabel('l-send', a3, 'raw bytes,\nor eval'),
    ack,
    text('l-ack', col(1) + W / 2 + back / -2 - ackW / 2, Y + H + 150, ackText, 18, { strokeColor: '#e03131' }),
    sink.el, sink.label,
    toMirror,
    text('l-mirror', col(0) + W + 24, Y + H + 212, 'a choked sink is skipped,\nnever waited on — it\ncatches up from here', 16, { strokeColor: '#868e96' }),
  ]
}

/** The file as `serializeAsJSON` would write it. */
export const DRAWING_JSON = JSON.stringify(
  {
    type: 'excalidraw',
    version: 2,
    source: 'https://excalidraw.com',
    elements: drawingElements(),
    appState: { gridSize: 20, gridStep: 5, gridModeEnabled: false, viewBackgroundColor: '#ffffff' },
    files: {},
  },
  null,
  2,
)

/**
 * `cide_ipc::frame` as `file_read_bytes` answers it: `u32 LE n | n bytes of JSON head | payload`.
 * The pane unpacks the `ArrayBuffer` itself (`client.ts::unpackFrame`), so the handler has to
 * hand it bytes, not an object — the one command in the demo that is not JSON.
 */
function frame(head: FileBytesHead, payload: Uint8Array): ArrayBuffer {
  const h = new TextEncoder().encode(JSON.stringify(head))
  const out = new Uint8Array(4 + h.length + payload.length)
  new DataView(out.buffer).setUint32(0, h.length, true)
  out.set(h, 4)
  out.set(payload, 4 + h.length)
  return out.buffer
}

const STAMP = { mtimeNanos: '1790000000000000000', len: 0 }

/**
 * The base tree with `crates/` folded and `docs/` open, so the two files the tabs hold are in
 * view beside them — the base world opens `cide-pty` instead, which pushes `docs/` to the bottom.
 */
function docsTree(): TreeRow[] {
  const docs = `${HOME}/docs`
  const rows = treeRows()
    .filter((r) => !r.path.startsWith(`${HOME}/crates/`))
    .map((r) => (r.path === `${HOME}/crates` ? { ...r, expanded: false } : r.path === docs ? { ...r, expanded: true } : r))
  const child = (name: string, kind: TreeRow['kind']): TreeRow => ({
    path: `${docs}/${name}`,
    name,
    depth: 1,
    kind,
    expanded: false,
    hasChildren: kind === 'dir',
    symlink: false,
    root: 0,
    detail: null,
  })
  const children = [
    child('adr', 'dir'),
    child('img', 'dir'),
    child('architecture.md', 'file'),
    child('checks.md', 'file'),
    child('journal.md', 'file'),
    child('platforms.md', 'file'),
    child('pty-backpressure.excalidraw', 'file'),
    child('pty-backpressure.md', 'file'),
    child('running.md', 'file'),
  ]
  const at = rows.findIndex((r) => r.path === docs)
  rows.splice(at + 1, 0, ...children)
  return rows
}

export function drawingHandlers(): Array<[string, Handler]> {
  const drawing = new TextEncoder().encode(DRAWING_JSON)
  const rows = docsTree()
  return [
    ['fs_tree_count', () => rows.length],
    ['fs_tree_rows', (a) => rows.slice(Number(a['offset']), Number(a['offset']) + Number(a['len']))],
    // Both files are new on this branch, and so is the folder's change.
    ['git_tree_status', (): TreeStatusMap => ({
      statuses: { ...TREE_STATUS, [NOTE_PATH]: 'untracked', [DRAWING_PATH]: 'untracked', [`${HOME}/docs`]: 'modified' },
      truncated: false,
    })],
    ['file_read', (a): FileDoc => {
      const path = String(a['path'])
      const body = path === NOTE_PATH ? NOTE_TEXT : ''
      return { path, text: body, writable: true, stamp: { ...STAMP, len: body.length } }
    }],
    ['file_read_bytes', (a) => {
      const path = String(a['path'])
      const bytes = path === DRAWING_PATH ? drawing : new Uint8Array()
      return frame({ writable: true, stamp: { ...STAMP, len: bytes.length } }, bytes)
    }],
    ['file_position', (a): ViewPosition | null => (String(a['path']) === NOTE_PATH ? NOTE_POSITION : null)],
    // The empty value of `FileOutline` is `null`, which the editor pane cannot take — it throws
    // on `.kind` and the pane falls back to its error boundary. `unsupported` is what a
    // language with no symbol source honestly answers.
    ['symbols_outline', (): FileOutline => ({ kind: 'unsupported', reason: 'no outline for markdown in the demo' })],
  ]
}
