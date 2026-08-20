/**
 * The blame gutter's hover card. (M19)
 *
 * Pure in the strong sense `panes/DiffPane.tsx` is: every value is a prop, there is no store, no
 * IPC and no clock. `blame.ts` builds the lines with `blameModel.popupLines`, mounts this into the
 * CodeMirror tooltip's DOM and supplies the two callbacks; nothing here knows that a repository
 * exists.
 *
 * # Why the body is a `string[]` and not seven props
 *
 * *Which rows the card has* is a decision — the `↳ was …` row appears only after a rename, the
 * boundary row only for a boundary commit, the downgrade note only when the follow mode was
 * weakened — and a decision spelled inside JSX is a decision no check script can run. So the rows
 * are `popupLines`' answer, which `check:blame` drives directly, and this file is only the part
 * that genuinely is a view. It is also what keeps the card and the marker's `title` from being two
 * renderings of the same commit that can disagree: they are the same array.
 *
 * The one index rule, stated in `popupLines` as well: **line 0 is the heading** — `a1b2c3d Fix the
 * parser` — and gets the mono face and the full-strength text colour. Everything after it is a
 * detail row.
 */
import type { ReactNode } from 'react'
import styles from './BlamePopup.module.css'

export interface BlamePopupProps {
  /**
   * Full oid of the commit this card is about, or `''` for an uncommitted line.
   *
   * The empty string is what removes the buttons. An uncommitted line has no commit to show in the
   * log and no parent to annotate, so offering either would be a control that reports a refusal
   * the user could have been spared — and this project's own rule is that a disabled row with a
   * guessed reason is worse than no row.
   */
  readonly oid: string
  /** The card's text, in order, from `blameModel.popupLines`. Line 0 is the heading. */
  readonly lines: readonly string[]
  readonly onShowCommit: () => void
  /**
   * Absent when the host has nowhere to put the answer, and then the button is not drawn.
   *
   * Optional rather than disabled: "annotate the previous revision" needs a read-only revision
   * tab to land in, and a build whose editor cannot open one has no useful refusal to report —
   * only an internal one. A disabled row with a guessed reason is the `repoOpen` mistake in
   * miniature, which this milestone's plan names twice.
   */
  readonly onAnnotateParent?: (() => void) | undefined
}

export function BlamePopup({
  oid,
  lines,
  onShowCommit,
  onAnnotateParent,
}: BlamePopupProps): ReactNode {
  const [head, ...rest] = lines
  return (
    <div className={styles.card}>
      <div className={styles.head}>{head ?? ''}</div>
      {rest.map((line, i) => (
        // The index is the key, and it is the right one here: this list is a rendering of one
        // immutable array that is replaced wholesale whenever the hovered line changes. There is
        // no reorder, no insert and no identity to preserve — the rows are positions, not things.
        // eslint-disable-next-line react/no-array-index-key
        <div className={styles.row} key={i}>
          {line}
        </div>
      ))}
      {oid !== '' && (
        <div className={styles.actions}>
          {/*
            * `type="button"`, both of them. A bare <button> inside anything that is ever a form is
            * a submit button, and these live in a floating card whose ancestor is not this file's
            * to know.
            */}
          <button type="button" className={styles.action} onClick={onShowCommit}>
            Show in log
          </button>
          {onAnnotateParent !== undefined && (
            <button type="button" className={styles.action} onClick={onAnnotateParent}>
              Annotate previous revision
            </button>
          )}
        </div>
      )}
    </div>
  )
}
