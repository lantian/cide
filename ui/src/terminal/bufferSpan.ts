/**
 * One logical line out of a terminal's buffer, and the arithmetic back to a cell. (M60)
 *
 * Shared by `runLinkProvider.ts` and `taskLinkProvider.ts`, which is the whole reason it is a
 * module: both need the same walk, and it lived private in the first of them until the second
 * arrived. Typed against local interfaces rather than xterm's own, so `check:task-links` can
 * compile this alone and drive it over a fake buffer — the arithmetic used to sit inside a DOM
 * callback, which is the one place no check script can reach, and it is the same mistake
 * `clickGate.ts` was created to undo.
 *
 * `IBuffer` and `IBufferLine` are assignable to [`SpanBuffer`] and [`SpanLine`] structurally, and
 * [`Span`] is `ILink['range']` spelled out, so no caller needs a cast.
 */

/**
 * How many wrapped rows a logical line may span before the walk gives up.
 *
 * A guard against a pathological buffer rather than a limit anybody should reach: a hover has to
 * answer in one frame, and a run of thousands of wrapped rows is a program printing a single
 * enormous line, in which case the link this walk was asked about is not the point.
 */
export const MAX_SPAN_ROWS = 64

/** One buffer row, as narrowly as the walk needs it. `IBufferLine` satisfies this. */
export interface SpanLine {
  readonly isWrapped: boolean
  translateToString(trim: boolean): string
}

/** The active buffer, as narrowly as the walk needs it. `IBuffer` satisfies this. */
export interface SpanBuffer {
  getLine(y: number): SpanLine | undefined
}

/** One logical line as the buffer holds it: the top row's index and every row, untrimmed. */
export interface LogicalLine {
  readonly top: number
  readonly rows: readonly string[]
}

/** A cell, 1-based on both axes — xterm's own coordinates. */
export interface SpanPoint {
  readonly x: number
  readonly y: number
}

/** `ILink['range']`, structurally: 1-based, `end` **inclusive**. */
export interface Span {
  readonly start: SpanPoint
  readonly end: SpanPoint
}

/**
 * The logical line the row `index` belongs to, following the terminal's own wrapping.
 *
 * Untrimmed rows (`translateToString(false)`), and that is load-bearing rather than incidental:
 * it is what makes a string offset into the joined text map back to a cell by dividing by the
 * width. `pathLinks.ts` trims and then corrects for it; a caller here does not have to.
 */
export function logicalLine(buffer: SpanBuffer, index: number): LogicalLine | null {
  if (!buffer.getLine(index)) return null
  let top = index
  while (top > 0 && index - top < MAX_SPAN_ROWS && buffer.getLine(top)?.isWrapped) top--
  let bottom = index
  while (bottom - top < MAX_SPAN_ROWS && buffer.getLine(bottom + 1)?.isWrapped) bottom++
  const rows: string[] = []
  for (let y = top; y <= bottom; y++) {
    const line = buffer.getLine(y)
    if (!line) break
    rows.push(line.translateToString(false))
  }
  return { top, rows }
}

/** Buffer coordinates, 1-based and inclusive as `ILink.range` wants them, for `[start, end)`. */
export function spanOf(logical: LogicalLine, cols: number, start: number, end: number): Span {
  const at = (offset: number): SpanPoint => ({
    x: (offset % cols) + 1,
    y: logical.top + Math.floor(offset / cols) + 1,
  })
  return { start: at(start), end: at(end - 1) }
}
