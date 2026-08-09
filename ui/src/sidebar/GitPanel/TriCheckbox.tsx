/**
 * The mock's 12px tri-state box: `✓` full, `–` partial, empty when off.
 *
 * Presentational only, and `aria-hidden`. The row that owns it is the `treeitem`, and that
 * row carries `aria-checked="true|false|mixed"` — a real `<input type=checkbox>` nested
 * inside a tree item would give screen readers two focusable things per row and make Space
 * ambiguous, while adding nothing a keyboard user does not already get from the row.
 */
import type { CheckState } from './model'
import styles from './TriCheckbox.module.css'

const GLYPH: Record<CheckState, string> = {
  checked: '✓',
  // U+2013 EN DASH, as the mock draws it — a hyphen is too short to read as "partial" at
  // 12px, and a minus sign is not in the UI font's coverage everywhere this ships.
  partial: '–',
  unchecked: '',
}

export function TriCheckbox({ state }: { state: CheckState }) {
  return (
    <span className={styles.box} data-state={state} data-audit="gitCheck" aria-hidden="true">
      {GLYPH[state]}
    </span>
  )
}
