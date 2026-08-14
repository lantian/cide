/**
 * The 42px activity rail down the left edge, below the header.
 *
 * The rail behaves like a vertical tablist — one selection at a time, each button swapping
 * the sidebar's contents — so it is marked up as one rather than as a toolbar. The items are
 * a data array rather than five hand-written blocks because later milestones add entries and
 * per-item adornments (the git count here) without disturbing the layout rules.
 *
 * Every value it draws is a prop and it reads no store, which is the rule for every component
 * in `chrome/`: it is what lets `chrome/layoutAudit.ts` drive each surface with fixed props
 * and measure the result. The git count is subscribed in `App.tsx` and passed down for that
 * reason and no other.
 */
import { Fragment } from 'react'
import { badgeLabel, badgeText } from '@/sidebar/GitPanel/model'
import { groupDigits } from '@/overlays/format'
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
  /**
   * Changed files across every repository in the project — `null` while none has been counted
   * yet, which is not the same as `0` and is drawn the same way for a different reason.
   *
   * A number rather than the `gitDirty` boolean this replaced. That boolean had exactly one
   * call site and it was `gitDirty={auditMode()}`: the badge was wired to the layout-audit
   * query flag and to nothing else, so in a normal launch it never rendered in any repository
   * state. See `chrome/gitCountStore.ts` for where the number now comes from.
   */
  changed?: number | null | undefined
  /**
   * Errors in the project, for the ⚑ badge. (M12)
   *
   * `null` means **nobody has looked** — no analyser is running, or none has answered yet — and
   * it draws no badge at all. That is the same distinction the panel and the status bar make, and
   * making it here too is what stops the rail from being the one surface that implies a clean
   * workspace: a `0` and a `null` must not look alike, so only a positive count draws anything.
   */
  errors?: number | null | undefined
  onSelect?: ((view: ActivityView) => void) | undefined
}

export function ActivityRail({ active, changed, errors, onSelect }: ActivityRailProps) {
  /*
   * Keyed by view id so the loop stays layout-only; more entries land here, not in JSX.
   *
   * Two values per entry, because the badge and its wording are no longer the same string: at
   * a hundred changed files the pill says `99+` and the button is still named "Git — 1,203
   * changed files". Both come from `GitPanel/model.ts` — the same module the panel's own repo
   * rows count with — so the rail and the panel cannot disagree about a number they both show.
   * The comma is this component's only contribution, because `model.ts` may not import
   * `groupDigits` and stay standalone-compilable for `check-git-tree.mjs`.
   */
  const count = changed ?? null
  const badges: Record<
    string,
    { pill: string; name: string; pillClass?: string | undefined } | undefined
  > = {
    git: (() => {
      const pill = badgeText(count)
      const name = count === null ? undefined : badgeLabel(count, groupDigits(count))
      return pill === null || name === undefined ? undefined : { pill, name }
    })(),
    // Only a positive, *known* count. `null` (nothing looked) and `0` (looked, clean) both draw
    // nothing — a badge is a call to action, and there is nothing to act on in either case.
    problems:
      typeof errors === 'number' && errors > 0
        ? {
            pill: badgeText(errors) ?? String(errors),
            pillClass: styles.badgeError,
            name: `${groupDigits(errors)} ${errors === 1 ? 'error' : 'errors'}`,
          }
        : undefined,
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
        const name = badge === undefined ? item.label : `${item.label} — ${badge.name}`
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
              {badge !== undefined && (
                /*
                 * `aria-hidden`, and it is not cosmetic. The badge used to be a 6px dot with
                 * no text, so it needed hiding from nothing; it now has content, and without
                 * this a screen reader reads the button as "Git — 3 changed files, 3" — and
                 * at the cap as "Git — 1,203 changed files, 99+", which is the one rendering
                 * of that number that means nothing at all. The name above is the wording.
                 */
                <span
                  className={`${styles.badge} ${badge.pillClass ?? ''}`}
                  data-audit="gitBadge"
                  aria-hidden="true"
                >
                  {badge.pill}
                </span>
              )}
            </button>
          </Fragment>
        )
      })}
    </div>
  )
}
