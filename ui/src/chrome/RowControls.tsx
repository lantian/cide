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
import styles from './AppHeader.module.css'

/** What the two buttons ask for. Exported so a check script can assert on the pair. */
export const ROW_INTENTS: ReadonlyArray<{
  id: string
  label: string
  title: string
  intent: SplitIntent
}> = [
  {
    id: 'bash',
    label: 'bash row',
    title: 'Add a full-width row holding a shell',
    intent: { kind: 'shell' },
  },
  {
    id: 'claude',
    label: 'claude row',
    title: 'Add a full-width row holding a new Claude session',
    intent: { kind: 'newClaude' },
  },
]

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
          <span aria-hidden="true">⊞</span> {label}
        </button>
      ))}
    </div>
  )
}
