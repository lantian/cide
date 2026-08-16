/**
 * The speed-search readout, and the highlight inside a matched name.
 *
 * One component for both sidebar trees. The bar is a *sibling* of the tree's scroller, never a
 * child — see `SpeedSearch.module.css` for the two reasons — and it is the only thing on screen
 * that says the tree is filtering rather than ignoring the keyboard.
 *
 * `role="status"` and not a `<label>` or an `<input>`: there is no field here, and inventing one
 * would take the caret off the scroller, which is the tree's single tab stop and the element
 * every key in this feature is delivered to.
 */
import type { ReactNode } from 'react'

import styles from './SpeedSearch.module.css'

export function SpeedSearchBar({
  query,
  summary,
  audit,
}: {
  query: string
  /** `3 of 17`, a dead-end sentence, or empty while the first frame is in flight. */
  summary: string
  /** `fileTreeSpeedSearch` / `gitTreeSpeedSearch`, per the panels' convention. */
  audit: string
}): ReactNode {
  return (
    <div className={styles.bar} data-audit={audit} role="status">
      <span className={styles.query}>{query}</span>
      {summary !== '' && <span className={styles.summary}>{summary}</span>}
    </div>
  )
}

/**
 * A row's name with the matched substring marked, or the name unchanged.
 *
 * Both trees draw their name through this, so the `<mark>` cannot end up spelled two ways — and
 * more importantly so the *span* cannot. `start` and `end` are UTF-16 code units, produced by
 * `cide_fs::speed` in that unit precisely because this is where they are used: `String.slice`
 * indexes UTF-16, and a byte offset would put the highlight four characters early on any name
 * containing an emoji. Clamped rather than trusted — the offsets crossed IPC, and a nonsensical
 * range here is a wrong-looking row rather than an exception inside a render.
 */
export function SpeedName({
  name,
  span,
}: {
  name: string
  span: { start: number; end: number } | undefined
}): ReactNode {
  if (span === undefined) return name
  const start = Math.max(0, Math.min(span.start, name.length))
  const end = Math.max(start, Math.min(span.end, name.length))
  if (end === start) return name
  return (
    <>
      {name.slice(0, start)}
      <mark className={styles.match}>{name.slice(start, end)}</mark>
      {name.slice(end)}
    </>
  )
}
