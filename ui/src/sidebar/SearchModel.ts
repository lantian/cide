/**
 * The pure half of the search panel: grouping, highlighting, the state machine and the
 * readout.
 *
 * Pure and import-free so `ui/scripts/check-search.mjs` can compile it standalone, the same
 * arrangement `rowWindow.ts` and `overlays/score.ts` are in — there is no JS test runner in
 * this project, and these are the parts of the panel where being wrong is invisible: an
 * off-by-one in a byte offset draws the highlight one character to the left, which looks like
 * a rendering quirk rather than a bug, and a grouping that reorders files turns a search over
 * a large repository into a list that shuffles while it fills.
 *
 * The DTO is not imported for the same reason. [`Hit`] is the structural subset this module
 * needs; `SearchStore.ts` holds a compile-time assertion that the generated `SearchHit`
 * satisfies it, so a field renamed in Rust fails `tsc` rather than drawing `undefined`.
 */

/** The fields of the generated `SearchHit` this module uses. */
export interface Hit {
  /** Absolute path — the identity a group is keyed on, and what opening the hit acts on. */
  path: string
  /** Path relative to its root. What the group header draws. */
  rel: string
  /** 1-based. */
  line: number
  /** The matching line, already clipped in Rust. */
  text: string
  /** Byte offsets of the match within `text`. See `splitHighlight`. */
  start: number
  end: number
}

/** A file heading, with the number of hits under it. */
export interface FileRow {
  kind: 'file'
  path: string
  rel: string
  /** Hits in this file, whether or not the group is collapsed. */
  hits: number
  collapsed: boolean
}

/** One matching line, under its file. */
export interface HitRow {
  kind: 'hit'
  /** Index into the flat hit list, so selecting a row can name it without a search. */
  index: number
  hit: Hit
}

export type SearchRow = FileRow | HitRow

/**
 * Flatten hits into the rows the virtualizer draws: one heading per file, then its hits.
 *
 * Grouping is by *consecutive run*, not by a map over the whole list. Rust sends every hit
 * from one file as one batch and never revisits a file, so consecutive runs are exactly the
 * files — and a run-based grouping cannot reorder anything, which is the property that
 * matters while results are still arriving. A `Map` keyed on the path would produce the same
 * groups today and would silently start moving a late hit up into an earlier group if the
 * backend ever scanned a file twice, which is a list that rearranges itself under the user's
 * cursor.
 */
export function groupHits(hits: readonly Hit[], collapsed: ReadonlySet<string>): SearchRow[] {
  const rows: SearchRow[] = []
  let index = 0
  while (index < hits.length) {
    const first = hits[index]
    if (first === undefined) break
    const path = first.path
    let end = index
    while (end < hits.length && hits[end]?.path === path) end += 1

    const hidden = collapsed.has(path)
    rows.push({
      kind: 'file',
      path,
      rel: first.rel,
      hits: end - index,
      collapsed: hidden,
    })
    if (!hidden) {
      for (let i = index; i < end; i += 1) {
        const hit = hits[i]
        if (hit !== undefined) rows.push({ kind: 'hit', index: i, hit })
      }
    }
    index = end
  }
  return rows
}

/** The three pieces a hit's line is drawn in: the match, and what surrounds it. */
export interface Highlight {
  before: string
  match: string
  after: string
}

/*
 * One encoder and one decoder for the whole module. Constructing a `TextEncoder` per row
 * would be an allocation per visible line per repaint, and they are stateless.
 */
const encoder = new TextEncoder()
const decoder = new TextDecoder()

/**
 * Split a line at the match's **byte** offsets.
 *
 * `text.slice(start, end)` is the obvious version and it is wrong: JS string indices are
 * UTF-16 units and the offsets from Rust are bytes. They agree for ASCII and diverge for
 * everything else — a line with one emoji before the match puts the highlight four characters
 * early, and a line with an accented word puts it one early. Both are silent.
 *
 * The ASCII fast path is not an optimisation for its own sake: almost every source line takes
 * it, and it keeps the slow path — encode, slice, decode — off the common case.
 *
 * Offsets are clamped rather than trusted. They arrive over IPC, and a `String.slice` with a
 * nonsensical range is a wrong-looking row while a decode of an out-of-range subarray is an
 * exception inside a render, which unmounts the whole panel.
 */
export function splitHighlight(text: string, start: number, end: number): Highlight {
  const bytes = encoder.encode(text)
  const lo = clamp(start, 0, bytes.length)
  const hi = clamp(end, lo, bytes.length)
  if (bytes.length === text.length) {
    // Pure ASCII: one byte per UTF-16 unit, so the offsets are already string indices.
    return { before: text.slice(0, lo), match: text.slice(lo, hi), after: text.slice(hi) }
  }
  return {
    before: decoder.decode(bytes.subarray(0, lo)),
    match: decoder.decode(bytes.subarray(lo, hi)),
    after: decoder.decode(bytes.subarray(hi)),
  }
}

function clamp(value: number, lo: number, hi: number): number {
  if (!Number.isFinite(value)) return lo
  return Math.min(Math.max(Math.trunc(value), lo), hi)
}

/**
 * Drop a line's leading indentation, moving the match offsets with it.
 *
 * A deeply indented hit would otherwise draw as an empty row in a 252px panel — the panel is
 * narrower than the indentation of a match inside a nested block. Only leading ASCII
 * whitespace is removed, so one stripped character is always one byte and the offsets shift
 * by the same amount in both units.
 */
export function trimIndent(hit: Hit): Hit {
  const trimmed = hit.text.replace(/^[ \t]+/, '')
  const shift = hit.text.length - trimmed.length
  if (shift === 0) return hit
  return {
    ...hit,
    text: trimmed,
    start: Math.max(0, hit.start - shift),
    end: Math.max(0, hit.end - shift),
  }
}

/**
 * What the panel is showing.
 *
 * `empty`, `searching` and `noResults` are three different things and the mock draws them
 * differently: no query at all is an invitation, a running walk is progress, and a finished
 * walk with nothing in it is an answer. Collapsing any two of them is how a search over a
 * large repository looks broken for the second it takes to find the first hit.
 */
export type PanelState = 'empty' | 'error' | 'searching' | 'noResults' | 'results'

export function panelState(input: {
  pattern: string
  running: boolean
  total: number
  error: string | null
}): PanelState {
  if (input.pattern === '') return 'empty'
  // The error outranks `running`: a pattern that did not compile started no walk, and a
  // panel that said "searching…" over an unparsable regex would wait for something that is
  // never going to happen.
  if (input.error !== null && input.error !== '') return 'error'
  if (input.total > 0) return 'results'
  return input.running ? 'searching' : 'noResults'
}

/**
 * The header's readout: `12 results in 3 files`.
 *
 * `format` is how the digit grouping gets in without this module importing anything — the
 * panel passes `groupDigits` from `overlays/format.ts`, and the default keeps the function
 * testable on its own. A truncated search reports its counts as floors (`5,000+`), because
 * the cap stopped the walk and the real totals are unknown rather than zero.
 */
export function summarize(
  total: number,
  files: number,
  truncated: boolean,
  format: (n: number) => string = String,
): string {
  if (total === 0) return 'no results'
  const count = truncated ? `${format(total)}+` : format(total)
  const results = total === 1 ? 'result' : 'results'
  const where = files === 1 ? 'file' : 'files'
  return `${count} ${results} in ${format(files)} ${where}`
}

/**
 * Whether two queries are the same search.
 *
 * The toggles are part of it. `spawn` with case-sensitivity off and on are two different
 * searches over one pattern, and a frame compared on the pattern alone would be painted
 * under the wrong toggle — which looks exactly like a search that ignored the toggle.
 */
export function sameQuery(a: QueryLike, b: QueryLike): boolean {
  return (
    a.pattern === b.pattern &&
    a.mode === b.mode &&
    a.caseSensitive === b.caseSensitive &&
    a.wholeWord === b.wholeWord
  )
}

/** The structural form of the generated `SearchQuery`; see the note on [`Hit`]. */
export interface QueryLike {
  pattern: string
  mode: 'literal' | 'regex'
  caseSensitive: boolean
  wholeWord: boolean
}

/** What the panel opens with. */
export const EMPTY_QUERY: QueryLike = {
  pattern: '',
  mode: 'literal',
  caseSensitive: false,
  wholeWord: false,
}
