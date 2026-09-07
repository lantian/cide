/**
 * Clickable tool lines in an agent run's pane. (M42)
 *
 * An xterm link provider over the buffer text: a line `runLinks.ts` recognises gets two links —
 * the `● tool` prefix and the `#7` token at the end — and either opens the whole event in
 * `LogDetailCard`, through the same `showLogDetail` road a shell pane's JSON log line uses.
 * The title between them is deliberately **not** linked: it is routinely a path
 * (`● read  src/main.rs`), and `pathLinks.ts`'s provider must keep it — xterm drops a later
 * provider's link wherever it overlaps an earlier one, and this provider registers after that
 * one.
 *
 * A **plain** click activates, unlike a path link's ctrl+click. Two reasons. The click gate in
 * `pathLinks.ts` claims every ctrl/meta+left press in a terminal pane in capture and resolves it
 * against *path* candidates, so a ctrl+click here would be answered "does not name a file"
 * before xterm ever saw it. And the gesture this stands in for is the OSC 8 timestamp link,
 * which xterm also activates on a plain click. A run's child prints lines and never turns
 * mouse tracking on, so the press reaches nothing that would misread it.
 *
 * `provideLinks` is called only when the pointer crosses into a buffer line it has not asked
 * about (see `pathLinks.ts`), so scanning a line here costs nothing per byte of output.
 */
import type { IBufferLine, IDisposable, ILink } from '@xterm/xterm'
import { paneSession } from '@/ipc/client'
import { showLogDetail } from '@/chrome/logDetailStore'
import { parseRunLine } from './runLinks'
import type { TerminalHandle } from './xterm'

/** How many wrapped rows a logical line may span before the walk gives up. */
const MAX_ROWS = 64

/** One logical line as the buffer holds it: the top row's index and every row, untrimmed. */
interface Logical {
  readonly top: number
  readonly rows: readonly string[]
}

/**
 * The logical line the row `index` belongs to, following the terminal's own wrapping.
 *
 * Untrimmed rows (`translateToString(false)`), so a string offset into the joined text maps
 * back to a cell by dividing by the width — `pathLinks.ts` trims and then corrects for it,
 * which a tool line, ASCII but for its glyph, does not need.
 */
function logicalLine(term: TerminalHandle['term'], index: number): Logical | null {
  const buf = term.buffer.active
  if (!buf.getLine(index)) return null
  let top = index
  while (top > 0 && index - top < MAX_ROWS && buf.getLine(top)?.isWrapped) top--
  let bottom = index
  while (bottom - top < MAX_ROWS && buf.getLine(bottom + 1)?.isWrapped) bottom++
  const rows: string[] = []
  for (let y = top; y <= bottom; y++) {
    const line: IBufferLine | undefined = buf.getLine(y)
    if (!line) break
    rows.push(line.translateToString(false))
  }
  return { top, rows }
}

/** Buffer coordinates, 1-based and inclusive as `ILink.range` wants them, for `[start, end)`. */
function rangeOf(logical: Logical, cols: number, start: number, end: number): ILink['range'] {
  const at = (offset: number) => ({
    x: (offset % cols) + 1,
    y: logical.top + Math.floor(offset / cols) + 1,
  })
  return { start: at(start), end: at(end - 1) }
}

export function attachRunLinks(
  handle: TerminalHandle,
  ctx: {
    /** The live session on this pane, read fresh — it is `null` until the child spawns. */
    readonly session: () => string | null
  },
): () => void {
  const term = handle.term
  const provider = {
    provideLinks(y: number, callback: (links: ILink[] | undefined) => void): void {
      const logical = logicalLine(term, y - 1)
      const session = ctx.session()
      if (logical === null || session === null) {
        callback(undefined)
        return
      }
      const text = logical.rows.join('')
      const parsed = parseRunLine(text)
      if (parsed === null) {
        callback(undefined)
        return
      }
      const cols = Math.max(1, term.cols)
      const { handle: lineHandle } = parsed
      const activate = () =>
        showLogDetail(() => paneSession.logDetail(session, lineHandle))
      const link = (start: number, end: number): ILink => ({
        range: rangeOf(logical, cols, start, end),
        text: text.slice(start, end),
        activate,
      })
      callback([link(0, parsed.toolEnd), link(parsed.tokenStart, parsed.end)])
    },
  }

  let registration: IDisposable | null = null
  try {
    registration = term.registerLinkProvider(provider)
  } catch {
    // A terminal disposed between `ensureTerminal` and here; there is nothing to register on.
  }
  return () => {
    try {
      registration?.dispose()
    } catch {
      // Disposing a provider on an already-disposed terminal throws; nothing to do about it.
    }
  }
}
