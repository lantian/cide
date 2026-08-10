/**
 * The minimap's arithmetic, with none of its painting.
 *
 * Split out of `minimap.ts` for one reason: everything here is decidable without a canvas,
 * a `ViewPlugin` or a DOM, and everything left there needs all three. A row that lands one
 * pitch off, a viewport rectangle that is a line too tall, a click that scrolls to the
 * wrong place — those are the failures the map actually has, they are pure functions of
 * four numbers, and while they lived inside a `PluginValue` nothing could reach them.
 *
 * The alternative was to export these from `minimap.ts` itself. It compiles, and the check
 * script would then have to import `@codemirror/view` under node to reach them — a module
 * whose top level reads `navigator` and `document`, and which is one release away from not
 * loading outside a browser at all. A file with no imports cannot acquire that problem.
 *
 * Everything here is in CSS pixels and 1-based document lines, matching CodeMirror.
 */

/** Total width of the minimap column, from the mock. */
export const MINIMAP_WIDTH = 96

/** Row pitch, and the bar drawn inside it. The mock says 3px bars. */
export const ROW_PITCH = 3
export const BAR_HEIGHT = 2

/** Horizontal padding inside the column, leaving room for the 1px rule on the left. */
const PAD_LEFT = 5
const PAD_RIGHT = 5

/**
 * Columns that map to the full drawable width.
 *
 * Not derived from the document's longest line: that would make the scale jump whenever a
 * long line scrolled into the window, and a minimap whose bars change length without the
 * text changing reads as broken. 110 is a little past this project's own line budget, so a
 * typical line uses most of the width and a genuinely long one clips.
 */
const SCALE_COLUMNS = 110

/**
 * How much of a line is inspected for its colour and its length.
 *
 * A minified bundle is one line of two megabytes; reading it whole, per line, per frame, is
 * the difference between a minimap and a hang. Past this the bar is drawn full width, which
 * is the truth about a line that long anyway.
 */
export const LINE_SCAN_LIMIT = 512

/** How many rows fit in a column `height` CSS pixels tall. Never fewer than one. */
export function mapRows(height: number): number {
  return Math.max(1, Math.floor(height / ROW_PITCH))
}

/**
 * The first document line the map shows, given how far the buffer is scrolled.
 *
 * A document that fits starts at line 1 and stays there; a longer one slides, so that the
 * top of the map is the top of the file at scroll 0 and the last screenful of the file at
 * the bottom. `scrollRange` is `scrollHeight - clientHeight` and is 0 for a document that
 * does not overflow its pane — in which case there is no fraction to compute and the answer
 * is the top.
 */
export function firstMapLine(
  totalLines: number,
  rows: number,
  scrollTop: number,
  scrollRange: number,
): number {
  if (totalLines <= rows) return 1
  const fraction = scrollRange > 0 ? Math.min(1, Math.max(0, scrollTop / scrollRange)) : 0
  return 1 + Math.round((totalLines - rows) * fraction)
}

/** The last document line the map shows, given the first and how many rows there are. */
export function lastMapLine(totalLines: number, rows: number, first: number): number {
  return Math.min(totalLines, first + rows - 1)
}

/** Where a row's bar sits: `y` is its top, `x`/`width` its horizontal extent. */
export interface BarRect {
  x: number
  y: number
  width: number
  height: number
}

/**
 * The bar for one line of the map.
 *
 * `indent` and `length` are in columns, from [`lineMetrics`]. The bar is clamped to the
 * drawable width from both ends rather than allowed to run under the padding, and is never
 * thinner than a pixel: a one-character line that rounds to 0.78px would otherwise vanish,
 * and a blank row and a row holding `}` look identical then.
 */
export function barRect(indent: number, length: number, rowIndex: number): BarRect {
  const usable = MINIMAP_WIDTH - PAD_LEFT - PAD_RIGHT
  const perColumn = usable / SCALE_COLUMNS
  const x = PAD_LEFT + Math.min(usable, indent * perColumn)
  const width = Math.max(1, Math.min(PAD_LEFT + usable - x, length * perColumn))
  return { x, y: rowIndex * ROW_PITCH, width, height: BAR_HEIGHT }
}

/** The `--sel` wash over the rows the buffer is actually showing. */
export interface ViewportRect {
  top: number
  height: number
}

/**
 * The viewport rectangle, or null when the visible range is outside the mapped window.
 *
 * The window and the viewport are computed from two different scroll readings — the map
 * slides on `scrollDOM.scrollTop`, the viewport is CodeMirror's own measured range — so
 * they are not guaranteed to overlap during a fling. Returning null rather than a rectangle
 * with negative height is what keeps a stray `fillRect` off the canvas.
 */
export function viewportRect(
  first: number,
  last: number,
  visibleFrom: number,
  visibleTo: number,
): ViewportRect | null {
  const top = (Math.max(first, visibleFrom) - first) * ROW_PITCH
  // `+ 1` because the last visible line is *shown*, so its whole row belongs inside.
  const bottom = (Math.min(last, visibleTo) - first + 1) * ROW_PITCH
  if (bottom <= top) return null
  return { top, height: bottom - top }
}

/** A line's indentation and content length, both in columns. */
export interface LineMetrics {
  indent: number
  length: number
}

/**
 * Indentation and trimmed content length for one line of text.
 *
 * Only the first [`LINE_SCAN_LIMIT`] characters are looked at, so a minified bundle costs
 * the same as a line of source; past that the bar is full width anyway, which is the truth
 * about a line that long. A line that is only whitespace reports length 0 and is not drawn
 * — a blank line in the map is what makes a block of code read as a block.
 */
export function lineMetrics(text: string, tabSize: number): LineMetrics {
  const scanned = text.length > LINE_SCAN_LIMIT ? text.slice(0, LINE_SCAN_LIMIT) : text

  let indent = 0
  while (indent < scanned.length && (scanned[indent] === ' ' || scanned[indent] === '\t')) indent++
  const length = scanned.trimEnd().length - indent
  if (length <= 0) return { indent: 0, length: 0 }

  // Tabs count for their visual width; a tab-indented file would otherwise look flat.
  const tabs = scanned.slice(0, indent).split('\t').length - 1
  return { indent: indent + tabs * (tabSize - 1), length }
}

/**
 * The document line a pointer `offsetY` pixels into the map is over.
 *
 * Clamped to the document at both ends, so a click in the empty space below the last row of
 * a short file scrolls to its end rather than off it.
 */
export function lineAtOffset(offsetY: number, first: number, totalLines: number): number {
  const offset = Math.floor(offsetY / ROW_PITCH)
  return Math.min(totalLines, Math.max(1, first + offset))
}
