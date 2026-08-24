/**
 * The ribbons between the two columns of a split diff. (M25)
 *
 * IDEA draws a filled shape in the gutter between the panes for every block that differs: it
 * joins the block's top and bottom on the left to the same block's top and bottom on the right,
 * so a run that is six lines on one side and one line on the other reads as a *taper* rather than
 * as two unrelated highlights. Where a side has no lines at all — a pure insertion — the shape
 * collapses to a point on that side, which is what makes an insertion legible as arriving from
 * *between* two lines rather than replacing one.
 *
 * That is the whole reason this exists. The columns are independently scrolled and independently
 * laid out (see `GitDiffPane.tsx`'s note on why the one-grid layout lost), so nothing about the
 * markup says which block on the left is which block on the right. `columnRows`' run table says
 * it exactly, and this turns that table plus two sets of measured row tops into paths.
 *
 * # Deliberately import-free
 *
 * `scripts/check-diff-render.mjs` compiles this standalone and asserts on the geometry, the way
 * it does `diffRows.ts` and `diffSync.ts` — whose header makes the same argument. The arithmetic
 * here is the kind that looks right and is off by one column of scroll, and none of it is visible
 * in any markup snapshot: a wrong path is a shape in the wrong place, not a missing element.
 * [`RunLike`] restates the fields of `diffRows.DiffRun` for the same reason `diffSync` does.
 */

/** The fields of a `diffRows.DiffRun` this module reads. */
export interface RunLike {
  readonly kind: 'shared' | 'add' | 'del' | 'pair'
  readonly leftFrom: number
  readonly leftTo: number
  readonly rightFrom: number
  readonly rightTo: number
}

/** One ribbon, ready to draw. */
export interface ConnectorShape {
  /** Which tone to fill it with. `pair` is a replacement — both sides have lines. */
  readonly kind: 'add' | 'del' | 'pair'
  /** An SVG path, in the connector's own coordinates: x from 0 to `width`, y in pixels. */
  readonly d: string
  /** The run this came from, so a hit test could name it. */
  readonly run: number
}

/**
 * How far a ribbon's edge runs flat before it starts to curve, as a fraction of the width.
 *
 * Entirely cosmetic and stated once. A pure cubic from edge to edge leaves the shape looking
 * hinged at the columns; a short flat run at each end makes it read as a band leaving one
 * document and arriving in the other, which is the shape IDEA draws.
 */
const FLAT = 0.18

/**
 * Ribbons for every run that differs, in file order.
 *
 * `leftTops[i]` is the top of column row `i` in that column's *content* coordinates, with one
 * extra entry at the end for the content's full height — exactly what `GitDiffPane`'s `rowTops`
 * produces, and the same array `diffSync.mapScroll` is fed. A run's span is therefore
 * `tops[from]` to `tops[to]`, and `from === to` — a side with no rows in this run — is a
 * zero-height span at the point the block belongs to, which is what tapers the shape.
 *
 * Scroll is subtracted here rather than by transforming the SVG, because the two columns can be
 * at different scroll offsets for a moment (the follower is written after the leader's event) and
 * one transform cannot express two.
 *
 * Shapes entirely outside `height` are dropped: a diff of forty thousand lines has thousands of
 * runs, and the browser is much happier being handed the dozen that are on screen. The margin is
 * generous rather than exact so a shape that is half off the top still draws its visible half.
 */
export function connectorShapes(
  runs: readonly RunLike[],
  leftTops: readonly number[],
  rightTops: readonly number[],
  leftScroll: number,
  rightScroll: number,
  width: number,
  height: number,
): ConnectorShape[] {
  const out: ConnectorShape[] = []
  const c1 = width * FLAT
  const c2 = width * (1 - FLAT)

  runs.forEach((run, index) => {
    if (run.kind === 'shared') return
    const lt = leftTops[run.leftFrom]
    const lb = leftTops[run.leftTo]
    const rt = rightTops[run.rightFrom]
    const rb = rightTops[run.rightTo]
    // A run whose boundary was never measured — the column has not laid out yet, or the tops
    // array is a render behind. Drawing from `undefined` would put `NaN` in the path, which SVG
    // renders as nothing at all *and* logs nothing, so it is skipped explicitly instead.
    if (lt === undefined || lb === undefined || rt === undefined || rb === undefined) return

    const ly0 = lt - leftScroll
    const ly1 = lb - leftScroll
    const ry0 = rt - rightScroll
    const ry1 = rb - rightScroll
    if (Math.max(ly1, ry1) < -height || Math.min(ly0, ry0) > height * 2) return

    // Two cubics and two straight edges: across the top from left to right, down the right
    // column's span, back across the bottom, up the left column's span.
    const d =
      `M 0 ${r(ly0)} ` +
      `C ${r(c1)} ${r(ly0)}, ${r(c2)} ${r(ry0)}, ${r(width)} ${r(ry0)} ` +
      `L ${r(width)} ${r(ry1)} ` +
      `C ${r(c2)} ${r(ry1)}, ${r(c1)} ${r(ly1)}, 0 ${r(ly1)} ` +
      `Z`
    out.push({ kind: run.kind, d, run: index })
  })
  return out
}

/**
 * One coordinate, rounded to a tenth of a pixel.
 *
 * Not cosmetic: these paths are rebuilt on every scroll frame and compared against the last ones
 * to decide whether to touch the DOM at all, so a coordinate that carries seventeen digits of
 * floating-point noise is a repaint per frame for a shape that did not move.
 */
function r(n: number): number {
  return Math.round(n * 10) / 10
}
