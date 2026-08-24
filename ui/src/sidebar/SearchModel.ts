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
 *
 * The one import is a **relative** one to a sibling that is itself import-free and compiled in
 * the same `tsc` invocation — the arrangement `chrome/panelRequests.ts` has with
 * `./sidebarView`. An `@/…` specifier would not resolve under a bare `tsc` with no `paths`,
 * which is the whole constraint. `rootOf` is imported rather than reimplemented because the
 * containment rule it encodes (longest match, segment-aware) is one this project has already
 * got wrong once; `rowPaths.ts`'s header is the account of that.
 */
import { rootOf } from './rowPaths'

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
 * A row's identity, for the panel's selection.
 *
 * A key rather than the row's position, because the positions move underneath it: results
 * stream in for seconds after the first ones paint, and folding a group away renumbers
 * everything below it. A selection stored as index 12 becomes a different line every time
 * either happens, which is worse than no selection at all.
 *
 * The hit's *index into the flat hit list* is what identifies it, not `path:line` — one line
 * can hold two matches, and they are two separate rows the user can move between.
 */
export function rowKey(row: SearchRow): string {
  return row.kind === 'file' ? `f:${row.path}` : `h:${row.index}`
}

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

/**
 * Fold one polled page into the hits already held.
 *
 * The backend answers `offset = min(asked, total)`, and the panel always asks from the end of
 * what it holds — so the answered offset is at most the held length, never past it, and the
 * two cases are:
 *
 * - `offset === held.length`, every ordinary poll: a plain append.
 * - `offset < held.length`: the job answering this project was replaced under the panel — a
 *   second window searching the same project restarted the walk — so its list is shorter than
 *   the local one and the page is the start of a *new* result set. Truncating to `offset`
 *   before appending resynchronises on the backend, which is the authority.
 *
 * Dropping the mismatched frame instead does not recover: the next poll asks from the same
 * unchanged length and gets the same short answer, for ever. That is what this used to do.
 *
 * Slicing to `offset` is also what makes a gap impossible, which is the property that matters
 * more than either case — a gap is a hit the user is never shown and never learns about.
 */
export function spliceHits<T>(held: readonly T[], offset: number, page: readonly T[]): T[] {
  const at = Number.isFinite(offset) ? Math.min(Math.max(Math.trunc(offset), 0), held.length) : 0
  return [...held.slice(0, at), ...page]
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

/** Where a hit is, in the units an editor's caret is placed with. */
export interface HitPosition {
  /** 1-based, straight from the wire. */
  line: number
  /** 1-based, in UTF-16 units — what a JS string is indexed by, and what CodeMirror counts. */
  column: number
  /** 1-based and exclusive: the caret position at the end of the match. */
  endColumn: number
}

/**
 * A hit as a caret position.
 *
 * > *"search result click doesn't point me to found place (should open file and select the
 * > line)"*
 *
 * Opening the file is only half of that; the other half is a position, and the position on
 * the wire is in the wrong units for one. `start`/`end` are **byte** offsets, because that is
 * what the search engine works in, and an editor's column is a character offset. They agree
 * for ASCII and diverge for everything else — one emoji before the match puts the caret three
 * characters past it, one accented word puts it one past — which is a caret that lands
 * *nearly* right and therefore looks like the editor's fault rather than this conversion's.
 *
 * Deliberately computed on the **untrimmed** hit. `trimIndent` exists so a deeply indented
 * line is readable in a 252px panel; the file on disk still has that indentation, and a caret
 * placed at the trimmed column would be one whole indent to the left of the match.
 *
 * 1-based to match `line`, which is 1-based because every editor and every compiler
 * diagnostic counts lines that way. Mixing the two bases in one struct is how an off-by-one
 * gets shipped, so both are 1-based and the host converts once.
 */
export function hitPosition(hit: Hit): HitPosition {
  const bytes = encoder.encode(hit.text)
  const lo = clamp(hit.start, 0, bytes.length)
  const hi = clamp(hit.end, lo, bytes.length)
  if (bytes.length === hit.text.length) {
    // Pure ASCII: one byte per UTF-16 unit, so the offsets are already columns.
    return { line: hit.line, column: lo + 1, endColumn: hi + 1 }
  }
  return {
    line: hit.line,
    column: decoder.decode(bytes.subarray(0, lo)).length + 1,
    endColumn: decoder.decode(bytes.subarray(0, hi)).length + 1,
  }
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
  // A match that starts inside the indentation is a match *on* the indentation — searching for
  // a tab, or for two spaces. Trimming it away would leave `start` and `end` both clamped to
  // 0, which draws the row with no highlight at all: the panel would show the hit and refuse
  // to say where it is. The row is left untrimmed instead, which is the only way the match can
  // still be pointed at.
  if (hit.start < shift) return hit
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
  error: ProblemLike | null
}): PanelState {
  if (input.pattern === '') return 'empty'
  // The error outranks `running`: an input that did not parse started no walk, and a panel
  // that said "searching…" over an unparsable regex would wait for something that is never
  // going to happen.
  if (input.error !== null) return 'error'
  if (input.total > 0) return 'results'
  return input.running ? 'searching' : 'noResults'
}

/** The structural form of the generated `SearchProblem`; see the note on [`Hit`]. */
export interface ProblemLike {
  kind: string
  detail: string
}

/** Which of the three boxes a problem is about, or `null` when it is about none of them. */
export type ProblemField = 'pattern' | 'scope' | 'include' | null

/**
 * The heading the notice draws above a problem's sentence.
 *
 * Here rather than on the wire: the kind is a fact about the search and the words are the
 * panel's copy, and a DTO carrying a heading would be a Rust crate deciding what a 252px box
 * says. The `never` is the point of the function — a fourth variant added in Rust becomes a
 * compile error here instead of a notice with a blank label over a sentence, which is what a
 * default arm would have produced.
 */
export function problemLabel(kind: string): string {
  switch (kind) {
    case 'pattern':
      return 'Bad pattern'
    case 'scope':
      return 'No such folder'
    case 'include':
      return 'Bad file pattern'
    default:
      // Not `never`-checked against a union, because [`ProblemLike`] is structural — the
      // exhaustiveness that matters is asserted in `SearchStore.ts`, where the generated
      // `SearchProblem` is narrowed. This is the runtime floor under a frame from a build
      // that knows a kind this one does not.
      return 'Search failed'
  }
}

/**
 * Which box to outline for a problem — the one the user can act on.
 *
 * Separate from [`problemLabel`] because the notice and the outline answer different
 * questions: the notice always says something, and an unrecognised kind must not put an
 * `aria-invalid` on an arbitrary input.
 */
export function problemField(kind: string): ProblemField {
  return kind === 'pattern' || kind === 'scope' || kind === 'include' ? kind : null
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
 * **Every field of `SearchQuery`, not just the pattern.** `spawn` with case-sensitivity off
 * and on are two different searches over one pattern, and a frame compared on the pattern
 * alone would be painted under the wrong toggle — which looks exactly like a search that
 * ignored the toggle. The scope and the glob are the same argument with a sharper edge: a
 * frame from the unscoped walk painted under a scoped box is one folder's results drawn under
 * another folder's heading, and there is nothing on screen to tell the user which it is.
 *
 * A field added in Rust and forgotten here is silent — the code still compiles, because this
 * takes a structural subset. `check-search.mjs` drives one field at a time against
 * `EMPTY_QUERY` to make it loud.
 */
export function sameQuery(a: QueryLike, b: QueryLike): boolean {
  return (
    a.pattern === b.pattern &&
    a.mode === b.mode &&
    a.caseSensitive === b.caseSensitive &&
    a.wholeWord === b.wholeWord &&
    a.scope === b.scope &&
    a.include === b.include
  )
}

/** The structural form of the generated `SearchQuery`; see the note on [`Hit`]. */
export interface QueryLike {
  pattern: string
  mode: 'literal' | 'regex'
  caseSensitive: boolean
  wholeWord: boolean
  /** The folder to search, or `''` for the whole project. See [`scopeLabel`]. */
  scope: string
  /** Comma-separated file globs, or `''` for every file. */
  include: string
}

/** What the panel opens with. Both narrowing boxes empty, which is today's whole-project search. */
export const EMPTY_QUERY: QueryLike = {
  pattern: '',
  mode: 'literal',
  caseSensitive: false,
  wholeWord: false,
  scope: '',
  include: '',
}

/** A project root, as much of one as [`scopeLabel`] needs. */
export interface RootLike {
  path: string
  label: string
}

/**
 * An absolute directory, as the string the scope box shows and the backend resolves.
 *
 * The spelling is deliberately [`Hit.rel`]'s: relative to the root that holds it, prefixed
 * with that root's label when the project has more than one. So the box reads
 * `cide-git/src` while the headings under it read `cide-git/src/log.rs`, and the two name one
 * place the same way. The absolute path would be truthful and unreadable — a 252px box shows
 * about thirty characters and `/home/…/work/cide/crates/cide-git` is not one of them.
 *
 * `cide_search::content::resolve_scope` is the other end of this and the only reader. The two
 * can drift, and the failure when they do is visible rather than silent: the panel reports
 * *No such folder* naming the string it sent, which is a bug report rather than a wrong answer.
 *
 * A path under no root answers with the **absolute path**, not `''`. Emptying the box would
 * silently widen the search back to the whole project, and a gesture that appears to do
 * nothing is worse than one that puts an unwieldy string in a box — the backend will say
 * *outside this project* about it, which is the true answer.
 */
export function scopeLabel(path: string, roots: readonly RootLike[]): string {
  const root = rootOf(
    path,
    roots.map((r) => r.path),
  )
  if (root === null) return path
  const owner = roots.find((r) => r.path === root)
  const label = owner === undefined ? '' : owner.label
  // The root itself is named by its label in either case. `''` would be the arithmetically
  // correct relative path and would read as an empty box — see above.
  if (root === path) return label
  const rel = path.slice(root.length + 1)
  return roots.length > 1 ? `${label}/${rel}` : rel
}

/**
 * The directory a tree row stands for: itself when it is one, its parent when it is a file.
 *
 * The frontend half of the rule `resolve_scope` also applies, and it is here so that the box
 * reads right the *instant* the gesture lands rather than after a round trip. Rust stays the
 * authority — the caller may not know a row's kind, because the tree windows its rows and the
 * selected one can be scrolled out of the cache — so this narrows when it can and hands the
 * path over untouched when it cannot.
 */
export function scopeDirOf(path: string, isDir: boolean): string {
  if (isDir) return path
  const cut = path.lastIndexOf('/')
  // No separator, or the root itself: there is no parent to name, so the path stands.
  return cut <= 0 ? path : path.slice(0, cut)
}
