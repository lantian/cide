/**
 * The mock's 12px tri-state box: `✓` full, `–` partial, empty when off.
 *
 * Presentational only, and `aria-hidden`. The row that owns it is the `treeitem`, and that
 * row carries `aria-checked="true|false|mixed"` — a real `<input type=checkbox>` nested
 * inside a tree item would give screen readers two focusable things per row and make Space
 * ambiguous, while adding nothing a keyboard user does not already get from the row.
 */
import type { CheckState } from './model'
import { Icon, type IconName } from '@/icons/Icon'

import styles from './TriCheckbox.module.css'

/*
 * The three states, as marks rather than as characters.
 *
 * `partial` used to be U+2013 EN DASH with a comment explaining that a hyphen was too short to
 * read at 12px and that a minus sign was not in the UI font's coverage everywhere this ships.
 * Both problems were the same problem — a glyph's shape and width belong to whichever face
 * fontconfig picked — and neither exists for a drawn mark. `minus` is a stroke on a 24-unit
 * grid: its length is a property of this app, not of the host's fonts.
 *
 * `unchecked` is `null`, not an empty mark: the box still occupies its column, which is what
 * keeps the rows aligned, but nothing is drawn in it.
 */
const MARK: Record<CheckState, IconName | null> = {
  checked: 'check',
  partial: 'minus',
  unchecked: null,
}

export function TriCheckbox({ state }: { state: CheckState }) {
  const mark = MARK[state]
  return (
    <span className={styles.box} data-state={state} data-audit="gitCheck" aria-hidden="true">
      {mark === null ? null : <Icon name={mark} size={0} />}
    </span>
  )
}
