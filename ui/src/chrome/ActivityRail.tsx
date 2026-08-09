/**
 * The 42px activity rail down the left edge, below the header.
 *
 * The rail behaves like a vertical tablist — one selection at a time, each button swapping
 * the sidebar's contents — so it is marked up as one rather than as a toolbar. The items are
 * a data array rather than five hand-written blocks because later milestones add entries and
 * per-item adornments (the git dot here, counts later) without disturbing the layout rules.
 */
import { Fragment } from 'react'
import styles from './ActivityRail.module.css'

export type ActivityView = 'files' | 'git' | 'search' | 'problems' | 'settings'

interface RailItem {
  id: ActivityView
  /* The mock's literal character. No icon font or SVG set is bundled to substitute for it. */
  glyph: string
  /* Per glyph, because the mock sizes each character separately to make them optically equal. */
  size: number
  label: string
  /* Renders after the flexible spacer, pinned to the foot of the rail. */
  bottom?: boolean
}

const ITEMS: readonly RailItem[] = [
  { id: 'files', glyph: '▤', size: 14, label: 'Files' },
  { id: 'git', glyph: '⑂', size: 14, label: 'Git' },
  { id: 'search', glyph: '⌕', size: 13, label: 'Search' },
  { id: 'problems', glyph: '⚑', size: 13, label: 'Problems' },
  { id: 'settings', glyph: '⚙', size: 13, label: 'Settings', bottom: true },
]

/* The spacer belongs immediately before the first pinned item, wherever the array puts it. */
const SPACER_AT = ITEMS.findIndex((item) => item.bottom === true)

export interface ActivityRailProps {
  active: ActivityView | null
  gitDirty?: boolean | undefined
  onSelect?: ((view: ActivityView) => void) | undefined
}

export function ActivityRail({ active, gitDirty, onSelect }: ActivityRailProps) {
  /*
   * Keyed by view id so the loop stays layout-only; more entries land here, not in JSX. The
   * value doubles as the badge's wording, because a 6px dot is invisible to a screen reader
   * unless it reaches the button's accessible name.
   */
  const badges: Record<string, string | undefined> = {
    git: gitDirty === true ? 'uncommitted changes' : undefined,
  }

  return (
    <div
      className={styles.rail}
      data-audit="rail"
      role="tablist"
      aria-orientation="vertical"
      aria-label="Activity"
    >
      {ITEMS.map((item, i) => {
        const selected = item.id === active
        const badge = badges[item.id]
        const name = badge === undefined ? item.label : `${item.label} — ${badge}`
        return (
          <Fragment key={item.id}>
            {/* Hidden from the accessibility tree: a tablist should own nothing but tabs,
                and this filler carries no meaning. */}
            {i === SPACER_AT && <div className={styles.spacer} aria-hidden="true" />}
            <button
              type="button"
              role="tab"
              aria-selected={selected}
              /* The glyph carries no text, so the tooltip is the only visible name and the
                 label is the only name assistive technology gets. */
              aria-label={name}
              title={name}
              className={selected ? `${styles.item} ${styles.itemActive}` : styles.item}
              data-audit="railIcon"
              onClick={() => onSelect?.(item.id)}
            >
              <span aria-hidden="true" style={{ fontSize: `${item.size}px` }}>
                {item.glyph}
              </span>
              {badge !== undefined && <span className={styles.badge} data-audit="gitBadge" />}
            </button>
          </Fragment>
        )
      })}
    </div>
  )
}
