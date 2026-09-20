/**
 * Which file tab shows a drawing, how its bytes are read, and what "dirty" means for a
 * canvas. (M63)
 *
 * > *"need implement support for .excalidraw, .excalidraw.json, .excalidraw.svg, and
 * > .excalidraw.png formats - when opening such file need to show an editor"*
 *
 * Pure and **import-free**, the same arrangement as `imageKinds.ts` and for the same reason:
 * `ui/scripts/check-excalidraw.mjs` compiles this file on its own with the TypeScript in
 * `node_modules` and drives every rule below under node. The component around it needs a
 * window, a Tauri host, a real file and a canvas to render at all, so the rules that decide a
 * file's fate live here where they can be asserted.
 *
 * # Three tables, three questions
 *
 * * `drawingKindFor` — does this **name** open in the drawing pane, and which format does a
 *   save write? Read by `PaneBody` before `imageKindFor`, because `x.excalidraw.png` ends in
 *   `.png` and the image viewer would otherwise take it; and by `keys/context.ts`, because a
 *   drawing pane is a `PaneKind::Editor` pane and must *not* count as `editorFocused`.
 * * `sniffDrawing` — what do these **bytes** hold? The name decides the pane and the write
 *   format; the bytes decide how to parse. Those disagree more often than seems likely: a
 *   plain screenshot renamed `.excalidraw.png`, a `.excalidraw` renamed `.excalidraw.png`
 *   before its first save, a JSON drawing somebody `cp`'d over an SVG one. Reading by name
 *   would render every one of those as an error; reading by bytes renders them, and the next
 *   save writes what the name says.
 * * `sceneFingerprint` — has the **scene** changed? Excalidraw's `onChange` fires for a
 *   selection, a pan, a zoom and a tool-colour change, and a tab that goes dirty when you look
 *   around a drawing is a close guard nobody trusts. What the file persists is the elements
 *   (`getSceneVersion` sums their versions, and a deletion bumps one), the embedded files, and
 *   exactly four `appState` keys — the ones `APP_STATE_STORAGE_CONF` marks `export: true` in
 *   0.18.1: `viewBackgroundColor`, `gridSize`, `gridStep`, `gridModeEnabled`. Nothing else is
 *   written, so nothing else may dirty the tab.
 *
 * The parsing traps `imageKindFor` lists apply here unchanged, plus one of its own: a
 * **dotfile** `.excalidraw` is a configuration file and not a drawing, so a name that *is* a
 * suffix with nothing in front of it is refused.
 */

/** The three formats a drawing is written in. Which one, the file's **name** decides. */
export type DrawingKind = 'json' | 'svg' | 'png'

/** What a file's leading bytes say it holds. `'empty'` is a file the tree just created. */
export type SniffedDrawing = 'json' | 'svg' | 'png' | 'empty'

/**
 * What the file tree's *New ▸ Drawing* seeds its name box with. (M64)
 *
 * One of the four [`SUFFIXES`] below, and the only one a *new* drawing should take: the file
 * is created empty, [`sniffDrawing`] answers `'empty'` for zero bytes, and the pane opens a
 * blank canvas — whereupon the first save writes the format the **name** implies. Plain JSON
 * is the one of the three that is nothing but the scene; the other two carry it inside a
 * raster or an XML document so that the file is also a picture, which is a thing to ask for
 * and not a default.
 *
 * Declared here and spelled into the table below rather than derived from it, because two of
 * the four arms are `'json'` and the obvious derivation — find the first `'json'` row —
 * answers `.excalidraw.json`.
 *
 * Note it is not itself a drawing: [`drawingKindFor`] wants a stem in front of the suffix, so
 * a file called exactly `.excalidraw` is a dotfile and routes nowhere. That is precisely the
 * state the seeded name box is in before the user types, which is why `FileTree` refuses to
 * commit a name that is still only the seed.
 */
export const DEFAULT_DRAWING_SUFFIX = '.excalidraw'

/**
 * Longest first, although order does not decide anything here: a name ending in
 * `.excalidraw.json` does not end in `.excalidraw`, so the four are disjoint.
 */
const SUFFIXES: readonly (readonly [suffix: string, kind: DrawingKind])[] = [
  ['.excalidraw.json', 'json'],
  ['.excalidraw.svg', 'svg'],
  ['.excalidraw.png', 'png'],
  [DEFAULT_DRAWING_SUFFIX, 'json'],
]

/** The last path segment on either separator, so `~/photos.png/notes.txt` is `notes.txt`. */
function basenameOf(path: string): string {
  const cut = Math.max(path.lastIndexOf('/'), path.lastIndexOf('\\'))
  return cut >= 0 ? path.slice(cut + 1) : path
}

/**
 * Which drawing format a path names, or `null` for a file that is not a drawing.
 *
 * Case-insensitive on the whole name. A name that is exactly a suffix (`.excalidraw`, a
 * dotfile) has no stem and is not a drawing; a trailing dot or a later suffix
 * (`x.excalidraw.png.bak`) is not either.
 */
export function drawingKindFor(path: string): DrawingKind | null {
  const name = basenameOf(path).toLowerCase()
  for (const [suffix, kind] of SUFFIXES) {
    if (name.length > suffix.length && name.endsWith(suffix)) return kind
  }
  return null
}

/** The PNG signature, RFC 2083 §3.1 — the eight bytes every PNG starts with. */
const PNG_SIGNATURE = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a] as const

/**
 * What the bytes hold.
 *
 * A PNG is its signature. Text is looked at after an optional UTF-8 byte-order mark and any
 * ASCII whitespace: `<` is an SVG (or the XML declaration in front of one), `{` is JSON, and
 * nothing at all is `'empty'` — a `.excalidraw` the tree's *New file* just created, which must
 * open as a blank canvas rather than as a parse error. Anything else is `null`, and the pane
 * says so by name.
 */
export function sniffDrawing(bytes: Uint8Array): SniffedDrawing | null {
  if (
    bytes.length >= PNG_SIGNATURE.length
    && PNG_SIGNATURE.every((byte, i) => bytes[i] === byte)
  ) {
    return 'png'
  }
  let at = 0
  if (bytes.length >= 3 && bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) at = 3
  while (at < bytes.length) {
    const b = bytes[at]
    if (b === 0x20 || b === 0x09 || b === 0x0a || b === 0x0d) at += 1
    else break
  }
  if (at >= bytes.length) return 'empty'
  const first = bytes[at]
  if (first === 0x3c) return 'svg'
  if (first === 0x7b) return 'json'
  return null
}

/**
 * The MIME type `loadFromBlob` sniffs on. It reads `blob.type` and nothing else to decide
 * between its three parsers, so the type on the `Blob` is what carries `sniffDrawing`'s answer
 * into Excalidraw.
 */
export function mimeFor(sniffed: Exclude<SniffedDrawing, 'empty'>): string {
  switch (sniffed) {
    case 'png':
      return 'image/png'
    case 'svg':
      return 'image/svg+xml'
    case 'json':
      return 'application/json'
  }
}

/**
 * The `appState` keys a saved drawing carries — `export: true` in Excalidraw 0.18.1's
 * `APP_STATE_STORAGE_CONF`, and nothing else. Read by `fingerprintOf` in the surface; pinned
 * by the check so a key added on one side is noticed on the other.
 */
export const PERSISTED_APP_STATE_KEYS = [
  'viewBackgroundColor',
  'gridSize',
  'gridStep',
  'gridModeEnabled',
] as const

/** The values behind `PERSISTED_APP_STATE_KEYS`, structurally, so this module imports nothing. */
export interface PersistedAppState {
  readonly viewBackgroundColor: string
  readonly gridSize: number
  readonly gridStep: number
  readonly gridModeEnabled: boolean
}

/** What a save writes, reduced to something comparable. */
export interface SceneFingerprint {
  /** `getSceneVersion(elements)` — the sum of every element's version. */
  readonly version: number
  /** The embedded files' ids, sorted, so insertion order cannot make two equal sets differ. */
  readonly files: string
  /** The four persisted `appState` values, joined. */
  readonly persisted: string
}

export function sceneFingerprint(
  version: number,
  fileIds: readonly string[],
  state: PersistedAppState,
): SceneFingerprint {
  return {
    version,
    files: [...fileIds].sort().join('\n'),
    persisted: [
      state.viewBackgroundColor,
      String(state.gridSize),
      String(state.gridStep),
      String(state.gridModeEnabled),
    ].join('\n'),
  }
}

export function sameFingerprint(a: SceneFingerprint | null, b: SceneFingerprint | null): boolean {
  if (a === null || b === null) return a === b
  return a.version === b.version && a.files === b.files && a.persisted === b.persisted
}

/**
 * `FileStamp`, structurally. `mtimeNanos` is a decimal **string** on the wire (see
 * `cide_ipc::FileStamp` for the arithmetic that made it one).
 */
export interface StampLike {
  readonly mtimeNanos: string
  readonly len: number
}

/**
 * Two stamps by **value**. Two stamps that arrived in two answers are two objects, and `===`
 * on them is never true — which would make every recheck a reload of a clean tab and a
 * conflict bar on a dirty one. `null` (a filesystem that would not answer) equals only `null`.
 */
export function sameStamp(a: StampLike | null, b: StampLike | null): boolean {
  if (a === null || b === null) return a === b
  return a.mtimeNanos === b.mtimeNanos && a.len === b.len
}

/** Binary units, matching the `MiB` the Rust refusals print — `imageKinds.formatBytes`'s rule. */
export function formatDrawingBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes < 0) return '—'
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KiB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`
}

/**
 * The status bar line while a drawing is in front — what replaces `Rust · UTF-8 · LF · Ln 7,
 * Col 48`. The format the file will be *written* in, the element count, and the size as read.
 */
export function drawingDetail(kind: DrawingKind, elements: number, bytes: number): string {
  const format = kind === 'json' ? 'Excalidraw' : `Excalidraw · ${kind.toUpperCase()}`
  const count = elements === 1 ? '1 element' : `${elements} elements`
  return `${format} · ${count} · ${formatDrawingBytes(bytes)}`
}

/**
 * The sentence for bytes that do not parse as a drawing.
 *
 * Names the file and carries Excalidraw's own reason, which is a sentence too
 * (*"Couldn't load invalid file"*, or the JSON parser's). Deliberately does not blame the
 * disk: the likeliest cause is a picture that was never a drawing, renamed.
 */
export function notADrawingMessage(name: string, reason: string): string {
  const why = reason.trim().replace(/[.\s]+$/, '')
  return why.length > 0
    ? `${name} does not hold an Excalidraw drawing: ${why}.`
    : `${name} does not hold an Excalidraw drawing.`
}
