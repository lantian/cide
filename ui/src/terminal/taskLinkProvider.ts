/**
 * Clickable task codes in a terminal pane. (M60)
 *
 * An xterm link provider over the buffer text: a `t-503` that is actually on this project's board
 * gets an underline on hover and opens the task's card on a plain click, through the same
 * `revealTask` the Agents panel's chips use. `taskLinks.ts` holds the whole rule — what a code is
 * and whether it may be offered — so this file is only the xterm and DOM half.
 *
 * # Third and last of cide's three providers
 *
 * xterm's `Linkifier._removeIntersectingLinks` drops a **later** provider's link wherever it
 * overlaps an earlier one, and `paneHosts.ts` registers this after the path links and the run
 * links. Both overlaps are deliberate. `pathMatch.ts`'s `BODY` includes `-`, so `t-503` is a
 * path *candidate*: a project that really does contain a file named `t-503` keeps its path link,
 * which is the right precedence — a file on disk is a fact and a code on the board is a guess
 * about what the text meant. And a code inside a tool line's title survives only because the run
 * provider deliberately claims the glyph and the `#7` and nothing between them.
 *
 * One consequence worth knowing: `pathLinks.ts` answers *asynchronously* (it probes the index
 * before it believes a path), so xterm cannot adopt this provider's synchronous answer at the
 * moment it is returned — it adopts it in the ordered fallback once every provider has replied.
 * A task code therefore lights up one round trip after the pointer crosses it. That is not a bug
 * and it is why this provider must not be made async as well.
 */
import type { IDisposable, ILink } from '@xterm/xterm'
import { revealTask } from '@/chrome/taskReveal'
import { useTasks } from '@/sidebar/tasksStore'
import { logicalLine, spanOf } from './bufferSpan'
import { offeredTaskCodes } from './taskLinks'
import type { TerminalHandle } from './xterm'

/**
 * What a pane needs to know before a task code in it means anything.
 *
 * Supplied by `TerminalPane` through [`setTaskLinkEnv`] rather than passed to
 * [`attachTaskLinks`], for `PathLinkEnv`'s stated reason: the attachment belongs to the host
 * (which outlives every mount) and this value belongs to the React props (which change without
 * the host changing).
 */
export interface TaskLinkEnv {
  /** The project this pane's output belongs to. `''` for a pane that has none. */
  readonly project: string
}

const envs = new Map<string, TaskLinkEnv>()

/** Called from `TerminalPane`'s effect. Passing `null` on cleanup makes the pane's codes inert. */
export function setTaskLinkEnv(paneId: string, env: TaskLinkEnv | null): void {
  if (env === null) envs.delete(paneId)
  else envs.set(paneId, env)
}

export function attachTaskLinks(
  handle: TerminalHandle,
  ctx: { readonly paneId: string },
): () => void {
  const term = handle.term
  const provider = {
    provideLinks(y: number, callback: (links: ILink[] | undefined) => void): void {
      const env = envs.get(ctx.paneId)
      const logical = logicalLine(term.buffer.active, y - 1)
      if (env === undefined || logical === null) {
        callback(undefined)
        return
      }
      /*
       * Read off the store, never subscribed to: this is a hover, there is nothing to re-render,
       * and a subscription here would wake every terminal pane in the window on every task
       * event.
       */
      const { project: boardProject, board } = useTasks.getState()
      const text = logical.rows.join('')
      /*
       * `offeredTaskCodes` and never `matchTaskCodes` — one road into the rule. The gate is the
       * half of this feature that decides whether a click can land, and a provider that matched
       * for itself would be free to draw an underline the rule says nothing may act on.
       */
      const codes = offeredTaskCodes(text, {
        paneProject: env.project,
        boardProject,
        board,
      })
      if (codes.length === 0) {
        callback(undefined)
        return
      }
      const cols = Math.max(1, term.cols)
      callback(
        codes.map((code) => ({
          range: spanOf(logical, cols, code.start, code.end),
          text: code.id,
          activate: (event: MouseEvent) => {
            /*
             * xterm's `Linkifier._handleMouseUp` (`Linkifier.ts:220`) checks neither the button
             * nor the selection: it activates whenever a mousedown and a mouseup landed on the
             * same link. So a right-click's mouseup opens this card behind the pane's own
             * context menu, a middle-click paste opens it instead of pasting, and dragging
             * across `t-503` to copy the id opens it too — three gestures nobody aimed here,
             * each of which reads as the app acting on its own.
             *
             * Ctrl and meta are refused as well, even though `pathLinks.ts` swallows that press
             * in capture long before xterm sees it. Stating the gesture here is what keeps the
             * two files from disagreeing if that gate ever changes.
             */
            if (event.button !== 0 || event.ctrlKey || event.metaKey) return
            if (term.getSelection() !== '') return
            revealTask(code.id, env.project === '' ? null : env.project)
          },
        })),
      )
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
