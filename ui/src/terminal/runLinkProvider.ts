/**
 * Clickable tool lines in an agent run's pane. (M42)
 *
 * An xterm link provider over the buffer text: a line `runLinks.ts` recognises gets two links —
 * the `● tool` prefix and the `#7` token at the end — and either opens the whole event in
 * `LogDetailCard`, through the same `showLogDetail` road a shell pane's JSON log line uses.
 * The title between them is deliberately **not** linked: it is routinely a path
 * (`● read  src/main.rs`), and `pathLinks.ts`'s provider must keep it — xterm drops a later
 * provider's link wherever it overlaps an earlier one, and this provider registers after that
 * one. Since M60 it is also what lets a task code in a title be clickable, because the third
 * provider registers after this one and inherits the same rule.
 *
 * That the *prefix* survives the same rule is not luck: `pathMatch.ts`'s `plausible` refuses a
 * candidate with fewer than two `/`-separated segments unless it is proven, so a bare word —
 * `bash`, and since M62 `thought` — is never a path candidate and the earlier provider never
 * claims the range.
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
 *
 * The buffer walk and the offset arithmetic live in `bufferSpan.ts`, shared with
 * `taskLinkProvider.ts` — see its header for why they are not private here any more.
 */
import type { IDisposable, ILink } from '@xterm/xterm'
import { paneSession } from '@/ipc/client'
import { showLogDetail } from '@/chrome/logDetailStore'
import { logicalLine, spanOf } from './bufferSpan'
import { parseRunLine } from './runLinks'
import type { TerminalHandle } from './xterm'

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
      const logical = logicalLine(term.buffer.active, y - 1)
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
      const activate = (event: MouseEvent) => {
        /*
         * xterm's `Linkifier._handleMouseUp` (`Linkifier.ts:220`) checks neither the button nor
         * the selection: it activates whenever a mousedown and a mouseup landed on the same
         * link. So a right-click's mouseup opened this card behind the pane's own context menu,
         * a middle-click paste opened it instead of pasting, and dragging across the `● bash`
         * glyph to copy the line opened it too — three gestures nobody aimed here, each of
         * which reads as the app acting on its own. (M60)
         *
         * Ctrl and meta are refused as well, even though `pathLinks.ts` swallows that press in
         * capture long before xterm sees it. Stating the gesture here is what keeps the two
         * files from disagreeing if that gate ever changes.
         */
        if (event.button !== 0 || event.ctrlKey || event.metaKey) return
        if (term.getSelection() !== '') return
        showLogDetail(() => paneSession.logDetail(session, lineHandle))
      }
      const link = (start: number, end: number): ILink => ({
        range: spanOf(logical, cols, start, end),
        text: text.slice(start, end),
        activate,
      })
      callback([link(parsed.toolStart, parsed.toolEnd), link(parsed.tokenStart, parsed.end)])
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
