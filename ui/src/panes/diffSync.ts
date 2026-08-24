/**
 * Keeping the split diff's two columns looking at the same change. (M25)
 *
 * The side-by-side view is two independently scrolling columns with no filler rows — the
 * left column simply has no rows where the right side's additions are, and a thin marker
 * between two of its lines says so (see [`insertMarkers`]). That means the columns are
 * different heights and a shared scroller cannot exist; this module is the mapping that
 * keeps them in step instead.
 *
 * # Anchored on runs, interpolated in pixels
 *
 * `diffRows.columnRows` emits a run table: `shared` runs occupy both columns with equal row
 * counts, and `add`/`del`/`pair` runs are where the columns disagree. The pane measures the
 * pixel top of each disagreeing run's boundary rows in both columns — once per render and
 * after a settled resize, never per scroll frame (`check:resize`'s rule; the reasoning is
 * `editor/markdown/scrollSync.ts`'s, which this module is patterned on) — bracketed by a
 * `(0, 0)` start sentinel and an end sentinel at the two content heights. Between anchors
 * the mapping interpolates proportionally: exact at every boundary, and in between off by
 * at most the wrap-difference of one shared run (the right column is narrower when its
 * blame track is on, so the same text can wrap differently — which is also why this cannot
 * be `rowIndex × lineHeight`).
 *
 * A zero-height *target* segment — a pure insertion on the other side — maps the whole
 * source segment to one pixel, which is the IDEA behaviour: the right column scrolls
 * through an inserted block while the left holds still at its marker.
 *
 * # The echo guard is the caller's, and it is a set of marks, not a timer
 *
 * Writing the other column's `scrollTop` fires that column's own `scroll` event,
 * asynchronously. `MergePane.tsx` documents at length why a time-released flag cannot guard
 * that loop (the flag is down before the echo arrives; three panes then converge on line
 * one), and its Set-of-marks guard is the pattern the pane reuses: mark the column you are
 * about to move, and its next scroll event consumes the mark and stops. The one trap this
 * module can name but not prevent: **clamp before comparing** — a write clamped to the
 * value already there fires no event, and a mark laid for it would linger and swallow the
 * next real scroll. [`mapScroll`] clamps so the caller's delta comparison is against the
 * value that would actually be written.
 *
 * # Why this module imports nothing
 *
 * `ui/scripts/check-diff-render.mjs` compiles it standalone and drives the arithmetic —
 * monotonicity, exactness at anchors, the zero-height segment, both clamps. [`RunLike`] is
 * a structural restatement of `diffRows.DiffRun`, kept honest by the typed call sites in
 * `GitDiffPane.tsx`.
 */

/** `diffRows.DiffRun`, restated. See the header for why this is not an import. */
export interface RunLike {
  readonly kind: 'shared' | 'add' | 'del' | 'pair'
  readonly leftFrom: number
  readonly leftTo: number
  readonly rightFrom: number
  readonly rightTo: number
}

/**
 * One disagreeing run's row ranges — the rows whose boundaries the pane measures.
 *
 * Only `add`, `del` and `pair` runs become spans: a `shared` run is the same rows in both
 * columns, so the offset across it carries and no anchor is needed inside it.
 */
export interface RowSpan {
  readonly leftFrom: number
  readonly leftTo: number
  readonly rightFrom: number
  readonly rightTo: number
}

export function rowSpans(runs: readonly RunLike[]): RowSpan[] {
  const out: RowSpan[] = []
  for (const run of runs) {
    if (run.kind === 'shared') continue
    out.push({
      leftFrom: run.leftFrom,
      leftTo: run.leftTo,
      rightFrom: run.rightFrom,
      rightTo: run.rightTo,
    })
  }
  return out
}

/** One measured correspondence: the same document position, as a pixel top in each column. */
export interface PairedAnchor {
  /** Pixel top in the left column's scroller. */
  readonly a: number
  /** Pixel top in the right column's scroller. */
  readonly b: number
}

/**
 * What the pane measured. `anchors` must be ascending on both axes and **bracketed**: the
 * first anchor is `(0, 0)` and the last is the two columns' content heights, so the mapping
 * is pure interpolation with no out-of-range regime. For span `k` the pane contributes the
 * pair at each boundary — the top of row `leftFrom`/`rightFrom` and of row
 * `leftTo`/`rightTo`, a row index equal to the column's length measuring as its content
 * height. `aMax`/`bMax` are `contentHeight - viewportHeight`, floored at zero.
 */
export interface SyncGeometry {
  readonly anchors: readonly PairedAnchor[]
  readonly aMax: number
  readonly bMax: number
}

/**
 * The other column's `scrollTop` for this one's. Monotonic, exact at every anchor, clamped
 * to what the target column can actually be scrolled to.
 */
export function mapScroll(geometry: SyncGeometry, from: 'a' | 'b', top: number): number {
  const { anchors } = geometry
  const limit = from === 'a' ? geometry.bMax : geometry.aMax
  if (anchors.length === 0) return Math.min(limit, Math.max(0, top))

  // The last anchor at or before `top` on the source axis. Binary search: this runs on
  // every frame of a scroll, and a long diff has an anchor pair per changed run.
  let lo = 0
  let hi = anchors.length - 1
  let index = 0
  while (lo <= hi) {
    const mid = (lo + hi) >> 1
    const anchor = anchors[mid]
    if (anchor === undefined) break
    const value = from === 'a' ? anchor.a : anchor.b
    if (value <= top) {
      index = mid
      lo = mid + 1
    } else {
      hi = mid - 1
    }
  }

  const anchor = anchors[index]
  if (anchor === undefined) return Math.min(limit, Math.max(0, top))
  const next = anchors[index + 1]
  const source = from === 'a' ? anchor.a : anchor.b
  const target = from === 'a' ? anchor.b : anchor.a
  if (next === undefined) {
    // Past the end sentinel (over-scroll, or an unbracketed table): carry the raw delta.
    return Math.min(limit, Math.max(0, target + (top - source)))
  }
  const sourceNext = from === 'a' ? next.a : next.b
  const targetNext = from === 'a' ? next.b : next.a
  const span = sourceNext - source
  // A zero-height *source* segment can only be hit exactly at its pixel; land on its start.
  const progress = span <= 0 ? 0 : Math.min(1, Math.max(0, (top - source) / span))
  return Math.min(limit, Math.max(0, target + progress * (targetNext - target)))
}

/**
 * Where the thin insertion lines go.
 *
 * The column that *lacks* a run gets the marker, at the boundary where the other column's
 * block sits: an `add` run puts a green line in the left column before its `leftFrom` row
 * (`leftFrom === leftTo` there, so either boundary names the same edge), a `del` run puts
 * the removal line in the right column, and a `pair` run gets none — both sides have
 * content, and the tint on the content is the signal. `beforeRow` may equal the column's
 * length: an insertion at end of file draws its marker after the last row.
 */
export interface InsertMarker {
  readonly column: 'left' | 'right'
  readonly beforeRow: number
  readonly tone: 'add' | 'del'
}

export function insertMarkers(runs: readonly RunLike[]): InsertMarker[] {
  const out: InsertMarker[] = []
  for (const run of runs) {
    if (run.kind === 'add') {
      out.push({ column: 'left', beforeRow: run.leftFrom, tone: 'add' })
    } else if (run.kind === 'del') {
      out.push({ column: 'right', beforeRow: run.rightFrom, tone: 'del' })
    }
  }
  return out
}
