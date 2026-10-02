/** Paths in terminal rows, including line breaks drawn by a harness rather than xterm. */
import {
  MAX_LINE, matchPaths, resolveCandidate, resolveDirectory,
  type Bases, type Candidate, type Resolution,
} from './pathMatch.js'

export const MAX_PATH_ROWS = 64
const MAX_RECONSTRUCTIONS = 128

export interface PathCell {
  getChars(): string
  getWidth(): number
}

export interface PathRow<Cell extends PathCell = PathCell> {
  readonly isWrapped: boolean
  readonly length: number
  translateToString(trim: boolean): string
  getCell(index: number, cell: Cell): unknown
}

export interface PathBuffer<Cell extends PathCell = PathCell> {
  getLine(index: number): PathRow<Cell> | undefined
  getNullCell(): Cell
}

interface Point { readonly x: number; readonly y: number }
export interface PathRange { readonly start: Point; readonly end: Point }

export interface PathLine {
  readonly text: string
  readonly top: number
  readonly bottom: number
  readonly starts: readonly Point[]
  readonly ends: readonly Point[]
}

/** Join native wrapped rows, without assuming that their text contains no spaces. */
export function readPathLine<Cell extends PathCell>(buffer: PathBuffer<Cell>, index: number): PathLine | null {
  if (!buffer.getLine(index)) return null
  let top = index
  while (top > 0 && buffer.getLine(top)?.isWrapped) {
    if (index - top >= MAX_PATH_ROWS - 1) return null
    top--
  }
  let bottom = index
  while (buffer.getLine(bottom + 1)?.isWrapped) {
    if (bottom - top >= MAX_PATH_ROWS - 1) return null
    bottom++
  }
  let text = ''
  const starts: Point[] = []
  const ends: Point[] = []
  const cell = buffer.getNullCell()
  for (let y = top; y <= bottom; y++) {
    const row = buffer.getLine(y)
    if (!row) return null
    const trimmed = row.translateToString(true)
    if (text.length + trimmed.length > MAX_LINE) return null
    let body = ''
    let printed = 0
    const rowStarts: Point[] = []
    const rowEnds: Point[] = []
    for (let x = 0; x < row.length; x++) {
      row.getCell(x, cell)
      const width = cell.getWidth()
      if (!width) continue
      const chars = cell.getChars() || ' '
      const start = { x: x + 1, y: y + 1 }
      const end = { x: x + width, y: y + 1 }
      body += chars
      if (cell.getChars() !== '') printed = body.length
      for (let n = 0; n < chars.length; n++) {
        rowStarts.push(start)
        rowEnds.push(end)
      }
      if (y === bottom && body.length >= trimmed.length) break
    }
    // A printed space at a native wrap boundary can be part of a quoted filename. Empty
    // cells, including the padding before an early-wrapped wide glyph, are not printed text.
    const length = y === bottom ? trimmed.length : printed
    if (text.length + length > MAX_LINE || rowStarts.length < length) return null
    text += body.slice(0, length)
    starts.push(...rowStarts.slice(0, length))
    ends.push(...rowEnds.slice(0, length))
  }
  return { text, top, bottom, starts, ends }
}

/** Offsets refer to the displayed text, before Markdown escapes are decoded. */
export function pathRange(line: PathLine, start: number, end: number): PathRange | null {
  const first = line.starts[start]
  const last = line.ends[end - 1]
  return first && last && end > start ? { start: first, end: last } : null
}

export interface BufferPath {
  readonly candidate: Candidate
  readonly ranges: readonly PathRange[]
  readonly reconstructed: boolean
  /** Includes the row the provider or click handler was asked about. */
  readonly relevant: boolean
}

/** A bounded neighborhood. Blank rows separate unrelated blocks of output. */
function nearbyLines<Cell extends PathCell>(buffer: PathBuffer<Cell>, current: PathLine): PathLine[] {
  const lines = [current]
  let rows = current.bottom - current.top + 1
  let length = current.text.length
  const add = (line: PathLine | null, before: boolean): boolean => {
    if (!line || !line.text.trim()) return false
    const count = line.bottom - line.top + 1
    if (rows + count > MAX_PATH_ROWS || length + line.text.length > MAX_LINE) return false
    rows += count
    length += line.text.length
    if (before) lines.unshift(line)
    else lines.push(line)
    return true
  }
  let previous = current.top - 1
  let next = current.bottom + 1
  let up = true
  let down = true
  while (rows < MAX_PATH_ROWS && (up || down)) {
    if (up) {
      const line = readPathLine(buffer, previous)
      up = add(line, true)
      if (line) previous = line.top - 1
    }
    if (down) {
      const line = readPathLine(buffer, next)
      down = add(line, false)
      if (line) next = line.bottom + 1
    }
  }
  return lines
}

// Backslashes are retained: an escape itself can straddle a hard line break.
const TAIL = /[A-Za-z0-9._+%~@/\\-]+$/
const HEAD = /^\s*([A-Za-z0-9._+%~@/\\-]+)/

/**
 * Ordinary candidates plus possible hard-wrapped paths covering the requested row.
 * Reconstructions are speculative until resolveBufferPaths verifies their full filename.
 */
export function bufferPaths<Cell extends PathCell>(buffer: PathBuffer<Cell>, current: PathLine): BufferPath[] {
  const out: BufferPath[] = []
  for (const candidate of matchPaths(current.text)) {
    const range = pathRange(current, candidate.start, candidate.end)
    if (range) out.push({ candidate, ranges: [range], reconstructed: false, relevant: true })
  }
  const ordinaryCount = out.length
  const lines = nearbyLines(buffer, current)
  for (let i = 0; i < lines.length; i++) {
    const first = lines[i]
    if (!first || first.top > current.top) break
    const tail = TAIL.exec(first.text)
    if (!tail || !tail[0].includes('/') || tail[0].endsWith('...')) continue
    let joined = first.text
    const parts: Array<{ line: PathLine; start: number; end: number }> = [
      { line: first, start: tail.index, end: first.text.length },
    ]
    for (let j = i + 1; j < lines.length; j++) {
      const line = lines[j]
      if (!line) break
      const head = HEAD.exec(line.text)
      const raw = head?.[1]
      // List bullets and truncation markers are boundaries, not filename continuations.
      if (!head || !raw || raw === '+' || raw === '-' || raw.startsWith('...')) break
      const start = head[0].length - raw.length
      const prefixLength = joined.length
      const complete = joined + line.text.slice(start)
      if (complete.length > MAX_LINE) break
      for (const candidate of matchPaths(complete)) {
        if (candidate.start < tail.index || candidate.start > tail.index + 2
          || candidate.end < prefixLength + raw.length) continue
        const fragments = parts.map((part, n) => pathRange(
          part.line, n === 0 ? candidate.start : part.start, part.end,
        ))
        fragments.push(pathRange(line, start, start + candidate.end - prefixLength))
        const ranges = fragments.filter((range): range is PathRange => range !== null)
        if (ranges.length === fragments.length) {
          out.push({ candidate, ranges, reconstructed: true, relevant: line.top >= current.top })
          // Dense logs can have hundreds of unrelated path tokens on adjacent rows. Refuse
          // their speculative joins as a whole instead of probing thousands of fake paths.
          if (out.length - ordinaryCount > MAX_RECONSTRUCTIONS) return out.slice(0, ordinaryCount)
        }
      }
      joined += raw
      parts.push({ line, start, end: start + raw.length })
      // Only a token ending at the row's edge can continue on another row.
      if (head[0].length !== line.text.length || raw.endsWith('...')) break
    }
  }
  return out
}

function compare(a: Point, b: Point): number {
  return a.y - b.y || a.x - b.x
}

function overlap(a: PathRange, b: PathRange): boolean {
  return compare(a.start, b.end) <= 0 && compare(b.start, a.end) <= 0
}

export interface ResolvedBufferPath extends BufferPath {
  readonly resolution: Resolution
  readonly isDirectory: boolean
}

/** Verified reconstructions outrank their pieces; competing full paths remain ambiguous. */
export function resolveBufferPaths(
  paths: readonly BufferPath[],
  bases: Bases,
  isFile: (path: string) => boolean,
  isDir: (path: string) => boolean,
): ResolvedBufferPath[] {
  const resolved = paths.map((path): ResolvedBufferPath => {
    const file = resolveCandidate(path.candidate.text, { ...bases, isFile })
    // A hard-wrapped directory is never proof that the printed filename was recovered.
    const dir = file.kind === 'none' && !path.reconstructed
      ? resolveDirectory(path.candidate.text, { ...bases, isDir }) : { kind: 'none' } as const
    return { ...path, resolution: file.kind === 'none' ? dir : file,
      isDirectory: dir.kind !== 'none' }
  })
  const groups: ResolvedBufferPath[][] = []
  for (const path of resolved.filter((p) => p.reconstructed && p.resolution.kind !== 'none')) {
    const touching = groups.filter((group) => group.some((p) =>
      p.ranges.some((a) => path.ranges.some((b) => overlap(a, b)))))
    const group = [path, ...touching.flat()]
    for (const previous of touching) groups.splice(groups.indexOf(previous), 1)
    groups.push(group)
  }
  const full = groups.filter((group) => group.some((path) => path.relevant))
    .map((group): ResolvedBufferPath => {
      const paths = new Set<string>()
      const ranges: PathRange[] = []
      for (const entry of group) {
        if (entry.resolution.kind === 'one') paths.add(entry.resolution.path)
        if (entry.resolution.kind === 'many') entry.resolution.paths.forEach((path) => paths.add(path))
        for (const range of entry.ranges) {
          if (!ranges.some((r) => compare(r.start, range.start) === 0
            && compare(r.end, range.end) === 0)) ranges.push(range)
        }
      }
      const targets = [...paths]
      const first = group.find((path) => path.relevant)!
      return { ...first, ranges, resolution: targets.length === 1
        ? { kind: 'one', path: targets[0]! } : { kind: 'many', paths: targets } }
    })
  return [...full, ...resolved.filter((path) => path.relevant && !full.some((p) =>
    p.ranges.some((a) => path.ranges.some((b) => overlap(a, b)))))
    .sort((a, b) => Number(b.reconstructed) - Number(a.reconstructed)
      || b.candidate.text.length - a.candidate.text.length)]
}
