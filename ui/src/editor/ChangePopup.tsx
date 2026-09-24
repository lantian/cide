/**
 * The change column's card. (M35)
 *
 * Pure in the strong sense `BlamePopup.tsx` is: every value is a prop, there is no store, no IPC
 * and no clock. `changeBars.ts` builds the content with `changeModel.popupContent`, mounts this
 * into the CodeMirror tooltip's DOM and supplies the callbacks; nothing here knows that a
 * repository exists.
 *
 * # Both actions are optional, and that is the shape rather than a `disabled` flag
 *
 * `onRevert` is absent on a read-only buffer, where the dispatch would be a silent no-op.
 * `onCopy` is absent for an added block, where HEAD had nothing to copy, and when the window's
 * capability file does not grant clipboard writes. In each case the button is **not drawn**,
 * which is `BlamePopupProps.onAnnotateParent`'s documented argument: a disabled row with a
 * guessed reason is worse than no row.
 */
import { Button } from '@/kit/components/Button'
import type { ReactNode } from 'react'
import styles from './ChangePopup.module.css'

export interface ChangePopupProps {
  /** `3 lines removed`, `2 lines changed`, … — from `changeModel.popupContent`. */
  readonly heading: string
  /** The HEAD lines, already truncated to `POPUP_MAX_LINES`. */
  readonly lines: readonly string[]
  /** How many more there were. Zero when nothing was cut. */
  readonly hiddenLines: number
  readonly onRevert?: (() => void) | undefined
  readonly onCopy?: (() => void) | undefined
}

export function ChangePopup({
  heading,
  lines,
  hiddenLines,
  onRevert,
  onCopy,
}: ChangePopupProps): ReactNode {
  return (
    <div className={styles.card}>
      <div className={styles.head}>{heading}</div>
      {lines.length > 0 && (
        <div className={styles.body}>
          {lines.map((line, i) => (
            // The index is the key, and it is the right one: this is a rendering of one immutable
            // array replaced wholesale whenever the card's block changes. No reorder, no insert,
            // no identity to preserve — the rows are positions, not things. Same call as
            // `BlamePopup`.
            // eslint-disable-next-line react/no-array-index-key
            <div className={styles.row} key={i}>
              {/* A blank line still needs a box, or the card silently shortens and the reader
                  cannot see that HEAD had an empty line there. */}
              {line === '' ? ' ' : line}
            </div>
          ))}
        </div>
      )}
      {hiddenLines > 0 && (
        <div className={styles.more}>
          {hiddenLines === 1 ? '1 more line' : `${hiddenLines} more lines`}
        </div>
      )}
      {(onRevert !== undefined || onCopy !== undefined) && (
        <div className={styles.actions}>
          {/* `type="button"`, both — the kit `Button` defaults to it. A bare <button> inside anything that is ever a form is a
              submit button, and these live in a floating card whose ancestor is not this file's
              to know. */}
          {onRevert !== undefined && (
            <Button size="sm"
              onClick={onRevert}
              title="Put HEAD's version of these lines back. Ctrl+Z undoes it."
            >
              Revert
            </Button>
          )}
          {onCopy !== undefined && (
            <Button size="sm" onClick={onCopy}>
              Copy
            </Button>
          )}
        </div>
      )}
    </div>
  )
}
