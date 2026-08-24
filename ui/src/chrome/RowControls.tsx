/**
 * `⊞ bash row` and `⊞ claude row`, moved out of the pane tree and into the header.
 *
 * The user asked for them "on top". They used to be a strip along the bottom of `SplitTree`,
 * which put the two most-used layout gestures in the app at the far edge of the window, below
 * whatever terminal happened to be tallest, and re-drew them once per tab.
 *
 * # Why this wires itself instead of taking a prop
 *
 * `AppHeader` is otherwise a pure render target. This is not, for the same reason `ProjectMenu`
 * is not: the row gesture is currently reachable only because `App.tsx` threads `onAddRow` into
 * `SplitTree`, and a control that moves to the header *and* waits for a new prop is a control
 * that ships dead. `onAddRow` is still honoured when a host passes one — that is how a fixture
 * or a future detached window drives it — but nothing has to.
 *
 * A row is anchored on the **focused pane**, not on the bottom of the tab, so a new row lands
 * under the row the user is working in. That is the behaviour `SplitTree`'s strip had and it is
 * preserved exactly; `pane_add_row` takes the anchor.
 */
import { useCallback } from 'react'
import type { SplitIntent } from '@/ipc/generated'
import { useWorkspace } from '@/store/workspace'
import { rowGate, rowTarget } from './menuModel'
import { Icon } from '@/icons/Icon'

import styles from './AppHeader.module.css'

/**
 * The two things a new pane can be, in the order the header offers them.
 *
 * **This is the app's one list of that choice, and it is here on purpose.** The pane title
 * bar's `⊞` now offers the same two kinds (`layout/PaneTitleBar.tsx`), and the brief for that
 * change said in as many words that this component is the precedent and its labels and intents
 * are the ones to match. A second literal there would be two lists that agree until somebody
 * edits one — which is already the situation with `SplitTree`'s own `ROW_INTENTS`, whose
 * wording drifted from this file's while both were describing the same gesture.
 *
 * `word` is the noun-less half of a label so a caller can say "bash row" or "bash pane" without
 * this file having to know which surface is asking; `what` is the object of the tooltip's
 * sentence, so both surfaces describe the same intent in the same words.
 */
export const PANE_KINDS: ReadonlyArray<{
  id: 'bash' | 'claude'
  word: string
  what: string
  intent: SplitIntent
}> = [
  { id: 'bash', word: 'bash', what: 'a shell', intent: { kind: 'shell' } },
  { id: 'claude', word: 'claude', what: 'a new Claude session', intent: { kind: 'newClaude' } },
]

/** What the two buttons ask for. Exported so a check script can assert on the pair. */
export const ROW_INTENTS: ReadonlyArray<{
  id: string
  label: string
  title: string
  intent: SplitIntent
}> = PANE_KINDS.map(({ id, word, what, intent }) => ({
  id,
  label: `${word} row`,
  title: `Add a full-width row holding ${what}`,
  intent,
}))

export interface RowControlsProps {
  /** Override. Absent — the ordinary case — the buttons drive the workspace store directly. */
  onAddRow?: ((intent: SplitIntent) => void) | undefined
}

export function RowControls({ onAddRow }: RowControlsProps) {
  const reason = useWorkspace((s) => (onAddRow ? null : rowGate(s.boot)))

  const add = useCallback(
    (intent: SplitIntent) => {
      if (onAddRow) {
        onAddRow(intent)
        return
      }
      // Re-read at click time rather than closing over a render-time target: the user may have
      // focused a different pane since this button last painted, and a row that lands under the
      // pane they *were* in is the kind of near-miss nobody reports and everybody works around.
      const target = rowTarget(useWorkspace.getState().boot)
      if (target === null) return
      void useWorkspace
        .getState()
        .addRow(target.project, target.tab, target.after, 'after', intent)
    },
    [onAddRow],
  )

  return (
    <div className={styles.rows} data-audit="headerRows">
      {ROW_INTENTS.map(({ id, label, title, intent }) => (
        <button
          key={id}
          type="button"
          className={styles.rowButton}
          data-row-intent={id}
          title={reason ?? title}
          aria-label={title}
          disabled={reason !== null}
          onClick={() => add(intent)}
        >
          <Icon name="rows-2" size={1} /> {label}
        </button>
      ))}
    </div>
  )
}
