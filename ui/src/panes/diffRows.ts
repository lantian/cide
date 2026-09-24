/**
 * The whole file, rebuilt from a patch and the new side's text. (M25)
 *
 * `git_diff_file` and `git_diff_revision` serve hunks at git's default three context lines —
 * they must, because `FileDiff.rev` hashes those exact patch bytes and staging re-derives
 * them — and since M25 they also carry both sides' full text. This module is the join: it
 * interleaves the wire hunks with the unchanged lines between them so the pane can draw the
 * document IDEA-style, whole, without the hunks themselves moving an inch.
 *
 * # The rule everything here obeys
 *
 * **Hunk rows are the wire rows, untouched.** A changed row's identity is `hunk:line` — the
 * pair a `LineRef` carries and staging resolves — so the reconstruction never invents,
 * reorders or renumbers a hunk line; it only places whole hunks into the stream and fills
 * the gaps from the text. A gap row has no position at all, which is what makes it
 * unselectable by construction rather than by a disabled flag someone could forget.
 *
 * # Validation, and why the answer to a mismatch is `null`
 *
 * The text and the patch are read in one backend round trip but travel as two claims, and
 * libgit2 filters the worktree side of a diff (crlf, ident) while the text is raw disk
 * bytes. CRLF is absorbed by [`splitLines`]' `\r` strip — the same rule as Rust's
 * `strip_newline` — but an ident-expanded keyword, or any drift this module did not
 * foresee, makes the gap lines and the hunk lines describe two different documents. Drawing
 * that would put wrong unchanged lines between correct changed ones, silently. So every
 * context and addition line is checked against the text, and any disagreement returns
 * `null`: the pane falls back to the hunks-only rendering, which is never wrong, only
 * shorter.
 *
 * # Deliberately import-free
 *
 * `scripts/check-diff-render.mjs` compiles this file standalone with the TypeScript in
 * `node_modules` and asserts on the output, the same way it does `diffBlame.ts` — whose
 * header explains the trade at length. [`RowLine`] and [`RowHunk`] are structural
 * restatements of the generated `DiffLineView`/`DiffHunkView`, kept honest by the typed
 * call sites in `GitDiffPane.tsx`.
 */

/** The wire's `DiffLineView`, restated. See the header for why this is not an import. */
export interface RowLine {
  readonly origin: 'context' | 'addition' | 'deletion'
  readonly content: string
  readonly oldLineno: number | null
  readonly newLineno: number | null
  readonly noNewline: boolean
}

/** The wire's `DiffHunkView`, restated. */
export interface RowHunk {
  readonly index: number
  readonly header: string
  readonly oldStart: number
  readonly oldLines: number
  readonly newStart: number
  readonly newLines: number
  readonly lines: readonly RowLine[]
}

/**
 * One stretch of the reconstructed file.
 *
 * A `hunk` segment is the wire hunk, by reference; a `gap` segment is the unchanged run
 * between two hunk coverages, synthesized from the text with **both** line numbers so the
 * split view can number either column from the same rows. `index` counts gaps from zero in
 * file order — it is the key expansion state is held under, so it must be stable across
 * re-renders of the same diff, which file order gives for free.
 */
export type FileSegment =
  | { readonly kind: 'hunk'; readonly hunk: RowHunk }
  | { readonly kind: 'gap'; readonly index: number; readonly lines: readonly RowLine[] }

/**
 * A [`FileSegment`], or a gap folded down to one row.
 *
 * `firstOld`/`firstNew` are where the fold starts, carried so a fold row can say where the
 * reader is — line numbers are the only landmark left once the lines are hidden.
 */
export type DisplaySegment =
  | FileSegment
  | {
      readonly kind: 'fold'
      readonly gap: number
      readonly count: number
      readonly firstOld: number
      readonly firstNew: number
    }

/**
 * Above this many total lines, long gaps start folded.
 *
 * The whole-file view's answer to `COLLAPSE_ABOVE`'s problem: there is no virtualisation in
 * this pane (deliberately — find-in-page, cross-hunk selection), so an unbounded render of a
 * 40,000-line file would lay out 40,000 nodes before the first frame. Below this the file is
 * simply all there, which is the point of the feature; above it the unchanged stretches fold
 * behind an expander row each, so the cost is bounded by what changed plus a row per fold.
 */
export const WHOLE_FILE_COLLAPSE_ABOVE = 5_000

/**
 * Gaps this short never fold, whatever the file's size. A fold row and the lines it hides
 * cost about the same at this length, and an expander that reveals three lines is a click
 * that bought nothing.
 */
export const GAP_COLLAPSE_MIN = 10

/**
 * A text into its lines, by the same rule Rust's `strip_newline` applies to hunk content:
 * split on `\n`, drop the phantom entry a trailing newline would create, strip one trailing
 * `\r` per line so CRLF content compares equal to libgit2's filtered hunk lines.
 */
export function splitLines(text: string): string[] {
  const parts = text.split('\n')
  if (parts.length > 0 && parts[parts.length - 1] === '') parts.pop()
  return parts.map((line) => (line.endsWith('\r') ? line.slice(0, -1) : line))
}

/**
 * Where a hunk's coverage begins and ends on one side, as 1-based inclusive line numbers —
 * empty when the hunk has no lines on that side.
 *
 * The zero-length convention is git's: `@@ -a,0 +c,d @@` numbers the *line before* the
 * insertion point, so an empty range at `start` means "between `start` and `start + 1`" and
 * the next line of that side is `start + 1`.
 */
function coverage(start: number, lines: number): { first: number; next: number } {
  if (lines === 0) return { first: start + 1, next: start + 1 }
  return { first: start, next: start + lines }
}

/**
 * The whole file as segments, or `null` when it cannot be trusted.
 *
 * `null` — the hunks-only fallback — on any of: no text at all (a deletion, a binary file,
 * an over-cap side, an old wire); hunks out of order or overlapping on the new side; a hunk
 * numbering past the end of the text; any context or addition line whose content is not the
 * text's line at its `newLineno`. The old side's numbers need no second text: inside a gap
 * the two documents are identical by definition, so the old number is the new one plus the
 * running offset the hunks themselves declare.
 */
export function wholeFileSegments(
  hunks: readonly RowHunk[],
  newText: string | null,
): FileSegment[] | null {
  if (newText === null) return null
  const text = splitLines(newText)
  const out: FileSegment[] = []
  let gapIndex = 0
  // 1-based cursors: the next unconsumed line on each side.
  let nextNew = 1
  let nextOld = 1

  const gapTo = (endNew: number): boolean => {
    // The unchanged run [nextNew, endNew], numbered on both sides. The two sides must be
    // the same length or the hunks' own arithmetic is inconsistent.
    if (endNew < nextNew - 1) return false
    const lines: RowLine[] = []
    for (let n = nextNew; n <= endNew; n += 1) {
      const content = text[n - 1]
      if (content === undefined) return false
      lines.push({
        origin: 'context',
        content,
        oldLineno: n + (nextOld - nextNew),
        newLineno: n,
        noNewline: false,
      })
    }
    if (lines.length > 0) {
      out.push({ kind: 'gap', index: gapIndex, lines })
      gapIndex += 1
    }
    nextOld += endNew + 1 - nextNew
    nextNew = endNew + 1
    return true
  }

  for (const hunk of hunks) {
    const newCover = coverage(hunk.newStart, hunk.newLines)
    const oldCover = coverage(hunk.oldStart, hunk.oldLines)
    // Out of order, overlapping, or disagreeing about the gap's length on the two sides.
    if (newCover.first < nextNew || oldCover.first < nextOld) return null
    if (newCover.first - nextNew !== oldCover.first - nextOld) return null
    if (!gapTo(newCover.first - 1)) return null
    // The hunk's own lines against the text: every row that exists on the new side must be
    // the text's row, or the two claims describe different documents.
    for (const line of hunk.lines) {
      if (line.origin === 'deletion') continue
      const at = line.newLineno
      if (at === null || text[at - 1] !== line.content) return null
    }
    out.push({ kind: 'hunk', hunk })
    nextNew = newCover.next
    nextOld = oldCover.next
  }
  if (!gapTo(text.length)) return null
  return out
}

/**
 * Segments with the folding policy applied.
 *
 * `totalLines` is the new side's line count — pass `splitLines(newText).length` — and
 * `expanded` holds the gap indices the user has opened. Folding replaces the gap, so
 * expansion is a re-render of the same pure call with one more index in the set; nothing is
 * mutated and nothing remembers DOM.
 */
export function presentSegments(
  segments: readonly FileSegment[],
  totalLines: number,
  expanded: ReadonlySet<number>,
  /**
   * Fold every long gap whatever the file's size. The GitLab review opts in: a reviewer reads
   * what the MR changed, and a 900-line file drawn whole buries a 4-line change in scrolling
   * (the user's complaint that the MR diff "shows the full file"). The working-tree diff keeps
   * the whole file below `WHOLE_FILE_COLLAPSE_ABOVE`, because there the surrounding code is
   * what you are about to stage against. No margin is kept around a fold: libgit2's hunks
   * already carry three lines of context, so a gap starts where the context ends.
   */
  foldAlways = false,
): DisplaySegment[] {
  if (!foldAlways && totalLines <= WHOLE_FILE_COLLAPSE_ABOVE) return [...segments]
  return segments.map((segment): DisplaySegment => {
    if (segment.kind !== 'gap') return segment
    if (segment.lines.length <= GAP_COLLAPSE_MIN) return segment
    if (expanded.has(segment.index)) return segment
    const first = segment.lines[0]
    return {
      kind: 'fold',
      gap: segment.index,
      count: segment.lines.length,
      firstOld: first?.oldLineno ?? 0,
      firstNew: first?.newLineno ?? 0,
    }
  })
}

/** Hunks alone as segments — the fallback's feed into [`columnRows`], no gaps to fill. */
export function hunkSegments(hunks: readonly RowHunk[]): FileSegment[] {
  return hunks.map((hunk) => ({ kind: 'hunk', hunk }))
}

// --- the split view's two columns ---------------------------------------------------------

/**
 * One row of one column of the side-by-side view.
 *
 * `line` rows draw through the pane's shared `cell()`; `bar` rows are the per-hunk strip
 * (staging box, or the `@@` header on the fallback); `fold` rows are the expander. `at` is
 * the `hunk:line` position and the old rule from the zipped layout survives verbatim:
 * **context positions ride the left column**, deletions the left, additions the right, so
 * every position occurs exactly once across both columns and "the rows drawn as selected"
 * stays a set of positions.
 */
export type ColumnRow =
  | { readonly kind: 'line'; readonly hunk: number; readonly at: number | null; readonly line: RowLine }
  | { readonly kind: 'bar'; readonly hunk: number }
  | { readonly kind: 'fold'; readonly gap: number; readonly count: number }

/**
 * One run of the diff, as end-exclusive row ranges into the two columns.
 *
 * `shared` runs — context, gaps, bars, folds — occupy both columns with **equal row
 * counts**; that equality is what lets the scroll sync treat everything between two changed
 * runs as one rigid block. `add` runs are empty on the left, `del` runs empty on the right,
 * and `pair` runs (a deletion run immediately followed by its addition run — one edit, per
 * git's `-` before `+` order) are nonempty on both sides with independent lengths. Nothing
 * is zipped and nothing fills: a side that has no lines in a run simply has no rows there,
 * which is the whole point of the layout.
 */
export interface DiffRun {
  readonly kind: 'shared' | 'add' | 'del' | 'pair'
  readonly leftFrom: number
  readonly leftTo: number
  readonly rightFrom: number
  readonly rightTo: number
}

export interface SplitColumns {
  readonly left: readonly ColumnRow[]
  readonly right: readonly ColumnRow[]
  readonly runs: readonly DiffRun[]
}

/** What [`columnRows`] draws beyond the lines themselves. */
export interface ColumnOptions {
  /**
   * Draw a `bar` row at each hunk's start, in both columns. On for the staging arm (the
   * tri-state box and the hunk action live there) and for the hunks-only fallback (the
   * `@@` header is the only landmark between discontinuous line numbers); off for a
   * read-only whole-file diff, where the bar would say nothing a line number does not.
   */
  readonly bars: boolean
  /**
   * Hunks whose lines are folded away, leaving only the bar. Only meaningful with `bars`
   * on — the fallback honours the same collapsed set the unified layout keeps.
   */
  readonly collapsed?: ReadonlySet<number>
}

/**
 * The two columns of the side-by-side view, and the run table that aligns them.
 *
 * Pure and exported so `check-diff-render.mjs` can assert on it standalone; the run table
 * is also what `diffSync.ts` builds its scroll anchors and insertion markers from, so the
 * three consumers — left column, right column, sync — cannot disagree about where a run
 * begins.
 */
export function columnRows(
  segments: readonly DisplaySegment[],
  options: ColumnOptions,
): SplitColumns {
  const left: ColumnRow[] = []
  const right: ColumnRow[] = []
  const runs: DiffRun[] = []
  const collapsed = options.collapsed ?? EMPTY_COLLAPSED

  const run = (kind: DiffRun['kind'], fill: () => void): void => {
    const leftFrom = left.length
    const rightFrom = right.length
    fill()
    if (left.length === leftFrom && right.length === rightFrom) return
    runs.push({ kind, leftFrom, leftTo: left.length, rightFrom, rightTo: right.length })
  }

  for (const segment of segments) {
    if (segment.kind === 'gap') {
      run('shared', () => {
        for (const line of segment.lines) {
          left.push({ kind: 'line', hunk: -1, at: null, line })
          right.push({ kind: 'line', hunk: -1, at: null, line })
        }
      })
      continue
    }
    if (segment.kind === 'fold') {
      run('shared', () => {
        left.push({ kind: 'fold', gap: segment.gap, count: segment.count })
        right.push({ kind: 'fold', gap: segment.gap, count: segment.count })
      })
      continue
    }
    const { hunk } = segment
    if (options.bars) {
      run('shared', () => {
        left.push({ kind: 'bar', hunk: hunk.index })
        right.push({ kind: 'bar', hunk: hunk.index })
      })
      if (collapsed.has(hunk.index)) continue
    }
    // The hunk's lines, grouped exactly as the old `splitHunk` grouped them: a deletion run
    // and the addition run immediately after it are one edit; `+` then `-` is two. The
    // difference is what happens to the group — ranges in a table now, never fillers.
    let dels: ColumnRow[] = []
    let adds: ColumnRow[] = []
    let ctxL: ColumnRow[] = []
    let ctxR: ColumnRow[] = []
    const flushCtx = (): void => {
      if (ctxL.length === 0) return
      const capL = ctxL
      const capR = ctxR
      run('shared', () => {
        for (const row of capL) left.push(row)
        for (const row of capR) right.push(row)
      })
      ctxL = []
      ctxR = []
    }
    const flushEdit = (): void => {
      if (dels.length === 0 && adds.length === 0) return
      const kind = dels.length > 0 && adds.length > 0 ? 'pair' : dels.length > 0 ? 'del' : 'add'
      const capDels = dels
      const capAdds = adds
      run(kind, () => {
        for (const row of capDels) left.push(row)
        for (const row of capAdds) right.push(row)
      })
      dels = []
      adds = []
    }
    hunk.lines.forEach((line, at) => {
      if (line.origin === 'deletion') {
        flushCtx()
        if (adds.length > 0) flushEdit()
        dels.push({ kind: 'line', hunk: hunk.index, at, line })
      } else if (line.origin === 'addition') {
        flushCtx()
        adds.push({ kind: 'line', hunk: hunk.index, at, line })
      } else {
        flushEdit()
        // The position rides on the left cell; the right one is the same text, unnumbered.
        ctxL.push({ kind: 'line', hunk: hunk.index, at, line })
        ctxR.push({ kind: 'line', hunk: hunk.index, at: null, line })
      }
    })
    flushEdit()
    flushCtx()
  }
  return { left, right, runs }
}

const EMPTY_COLLAPSED: ReadonlySet<number> = new Set<number>()

/**
 * Where one change begins or ends, addressed the way both layouts address a row.
 *
 * `hunk:at` is the pair a `LineRef` carries and staging resolves, and it is the **only**
 * identity the three renderings share — the split columns, the whole-file unified body and
 * the hunks-only fallback all put it on the row as `data-at`. A row index would not do: the
 * unified body nests gap rows inside a per-segment element and has no flat row list at all.
 */
export interface ChangeRowRef {
  readonly hunk: number
  readonly at: number
  readonly column: 'left' | 'right'
}

/**
 * One changed run — one edit.
 *
 * A `pair` counts **once**: a deletion run and the addition run immediately after it are one
 * edit, which is the grouping [`columnRows`] already performs and which git's `-`-before-`+`
 * ordering is what makes detectable. Stepping per hunk would be coarser than this and stepping
 * per line finer; a run is the unit a reader means by "the next change".
 */
export interface ChangeAnchor {
  readonly kind: 'add' | 'del' | 'pair'
  /** Index into [`SplitColumns.runs`] — the join to `rowSpans` and `insertMarkers`. */
  readonly run: number
  readonly first: ChangeRowRef
  readonly last: ChangeRowRef
  readonly leftFrom: number
  readonly leftTo: number
  readonly rightFrom: number
  readonly rightTo: number
}

/**
 * Every change in the diff, in file order.
 *
 * A **fourth consumer of the run table**, which is the property [`columnRows`]' comment claims
 * for the existing three: the left column, the right column and `diffSync.ts` cannot disagree
 * about where a run begins, and now neither can the iterator. Enumerating changes from
 * `segments` instead would be a second implementation of the deletion/addition grouping — the
 * duplication this module's header refuses — and it would answer differently the moment
 * `flushEdit`'s rule changed.
 *
 * Because the caller builds the columns from whatever it is *drawing* — collapsed hunks skipped,
 * gaps folded — the anchors are exactly the changes currently in the DOM. That is what makes
 * "Next change never scrolls to something invisible" true by construction rather than by a
 * filter someone has to remember to apply.
 */
export function changeAnchors(columns: SplitColumns): ChangeAnchor[] {
  const out: ChangeAnchor[] = []
  columns.runs.forEach((run, index) => {
    if (run.kind === 'shared') return
    // A `del` run lives entirely on the left, an `add` run entirely on the right, and a `pair`
    // begins on the left (the deletion) and ends on the right (the addition). Reading the
    // boundary rows rather than assuming them keeps this honest if the layout ever changes.
    const first =
      run.kind === 'add'
        ? rowRef(columns.right, run.rightFrom, 'right')
        : rowRef(columns.left, run.leftFrom, 'left')
    const last =
      run.kind === 'del'
        ? rowRef(columns.left, run.leftTo - 1, 'left')
        : rowRef(columns.right, run.rightTo - 1, 'right')
    // Defensive, and unreachable: `flushEdit` pushes only `line` rows and every one of them
    // carries a position on the column it rides. `--noUncheckedIndexedAccess` asks anyway, and a
    // run with no addressable boundary is one the iterator could not scroll to.
    if (first === null || last === null) return
    out.push({
      kind: run.kind,
      run: index,
      first,
      last,
      leftFrom: run.leftFrom,
      leftTo: run.leftTo,
      rightFrom: run.rightFrom,
      rightTo: run.rightTo,
    })
  })
  return out
}

/** One column row as a `hunk:at` reference, or `null` when it carries no position. */
function rowRef(
  rows: readonly ColumnRow[],
  index: number,
  column: 'left' | 'right',
): ChangeRowRef | null {
  const row = rows[index]
  if (row === undefined || row.kind !== 'line' || row.at === null) return null
  return { hunk: row.hunk, at: row.at, column }
}

/**
 * Every `hunk:at` inside one run.
 *
 * The set the view paints as current. Separate from [`changeAnchors`] and called for **one**
 * anchor at a time: a diff of forty thousand lines has thousands of changes, and building a
 * position list for every one of them on every render — to use exactly one — is the kind of
 * cost that only shows up on the file somebody actually needs this for.
 *
 * Both columns, because a `pair` is one change with rows on either side and marking only the
 * additions would read as two.
 */
export function changePositions(
  columns: SplitColumns,
  anchor: ChangeAnchor,
): Array<{ hunk: number; at: number }> {
  const out: Array<{ hunk: number; at: number }> = []
  const take = (rows: readonly ColumnRow[], from: number, to: number): void => {
    for (let i = from; i < to; i++) {
      const row = rows[i]
      if (row === undefined || row.kind !== 'line' || row.at === null) continue
      out.push({ hunk: row.hunk, at: row.at })
    }
  }
  take(columns.left, anchor.leftFrom, anchor.leftTo)
  take(columns.right, anchor.rightFrom, anchor.rightTo)
  return out
}
