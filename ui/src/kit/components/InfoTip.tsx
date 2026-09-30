/**
 * An explanation on demand: a small (i) beside a label that opens a popover of prose. (M133)
 *
 * # Why this exists
 *
 * Settings drew every explanation in full, under every label, and between the rows as blue and
 * amber `Note` blocks. The user's report was that the screen was hard to read and that a note
 * sitting between two rows did not say which of them it was about. The explanations were right
 * and mostly worth keeping; what was wrong was paying for all of them at once. So a row keeps one
 * line of hint, and the rest of what it has to say goes behind this mark.
 *
 * # How it opens
 *
 * - **Hover**, after `OPEN_DELAY`, so a pointer crossing the page on its way somewhere else does
 *   not flash a popover at every row it passes.
 * - **Focus**, at once: a keyboard user Tabbing through the form reads each explanation as they
 *   reach it, and a screen reader gets the same text through `aria-describedby`.
 * - **Click pins it.** A pinned popover stays when the pointer leaves, so a long paragraph can be
 *   read without holding the mouse still, and its text can be selected and copied — an env var
 *   name is the thing most often copied out of one. Escape, a second click or a mouse-down
 *   elsewhere closes it.
 *
 * The popover holds no controls, on purpose. It is portalled to `<body>`, which puts it last in
 * the tab order, far from the mark that opened it; a button inside it would be one a keyboard
 * user could not reach. An act that belongs to a row (Reset to default) is on the row instead.
 *
 * Placement is `Select`'s: measured from the trigger's rectangle, `position: fixed`, re-measured
 * on scroll and resize, and flipped upward when there is less room below than above. It is also
 * clamped sideways, because the mark sits at the end of a label and a 340px popover opened from
 * a label near the right edge would otherwise hang off the window.
 */
import {
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type CSSProperties,
  type ReactElement,
  type ReactNode,
} from 'react'
import { createPortal } from 'react-dom'

import { Icon } from '@/icons/Icon'
import styles from './InfoTip.module.css'

/** How long the pointer rests on the mark before it opens. */
const OPEN_DELAY = 300
/** How long it survives the pointer leaving — enough to cross the gap onto the popover. */
const CLOSE_DELAY = 160
/** Gap between the mark and the popover, and the popover's floor distance from the window edge. */
const GAP = 6
/** Room below the mark the popover wants before it prefers opening upward. */
const PREFERRED_ROOM = 220

export function InfoTip({
  label,
  children,
}: {
  /** The mark's accessible name: "About Show ignored files". */
  label: string
  children: ReactNode
}): ReactElement {
  const id = useId()
  const [open, setOpen] = useState(false)
  const [pinned, setPinned] = useState(false)
  const [place, setPlace] = useState<CSSProperties>({ visibility: 'hidden' })
  const trigger = useRef<HTMLButtonElement>(null)
  const popup = useRef<HTMLDivElement>(null)
  const timer = useRef<number | null>(null)

  const clearTimer = (): void => {
    if (timer.current !== null) window.clearTimeout(timer.current)
    timer.current = null
  }
  const later = (fn: () => void, ms: number): void => {
    clearTimer()
    timer.current = window.setTimeout(fn, ms)
  }
  const close = (): void => {
    clearTimer()
    setOpen(false)
    setPinned(false)
  }
  const hoverIn = (): void => later(() => setOpen(true), OPEN_DELAY)
  const hoverOut = (): void => {
    if (pinned) return clearTimer()
    later(() => setOpen(false), CLOSE_DELAY)
  }

  useEffect(() => clearTimer, [])

  useLayoutEffect(() => {
    if (!open) return
    const measure = (): void => {
      const r = trigger.current?.getBoundingClientRect()
      const w = popup.current?.offsetWidth ?? 0
      if (r === undefined) return
      const below = window.innerHeight - r.bottom
      const up = below < PREFERRED_ROOM && r.top > below
      const left = Math.max(GAP, Math.min(r.left - GAP, window.innerWidth - w - GAP))
      setPlace({
        left,
        ...(up ? { bottom: window.innerHeight - r.top + GAP } : { top: r.bottom + GAP }),
      })
    }
    measure()
    window.addEventListener('resize', measure)
    window.addEventListener('scroll', measure, true)
    return () => {
      window.removeEventListener('resize', measure)
      window.removeEventListener('scroll', measure, true)
    }
  }, [open])

  // Escape closes; while pinned, so does a mouse-down anywhere but the mark or the popover.
  useEffect(() => {
    if (!open) return
    const key = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      // Stopped here so the Escape that dismisses an explanation does not also close whatever
      // the explanation sits in — a dialog, the detached settings frame.
      e.stopPropagation()
      close()
    }
    const away = (e: MouseEvent): void => {
      const t = e.target as Node
      if (trigger.current?.contains(t) === true || popup.current?.contains(t) === true) return
      close()
    }
    document.addEventListener('keydown', key, true)
    document.addEventListener('mousedown', away)
    return () => {
      document.removeEventListener('keydown', key, true)
      document.removeEventListener('mousedown', away)
    }
  }, [open])

  return (
    <>
      <button
        ref={trigger}
        type="button"
        className={styles.mark}
        aria-label={label}
        aria-expanded={open}
        aria-describedby={id}
        data-pinned={pinned || undefined}
        onMouseEnter={hoverIn}
        onMouseLeave={hoverOut}
        onFocus={() => {
          clearTimer()
          setOpen(true)
        }}
        onBlur={() => {
          if (!pinned) close()
        }}
        onClick={() => {
          clearTimer()
          if (pinned) return close()
          setOpen(true)
          setPinned(true)
        }}
      >
        <Icon name="info" size={1} />
      </button>
      {/* The description a screen reader hears is this inline copy, not the popover: the
          popover exists only while open (a portal cannot be rendered where there is no
          `document`, which is where the kit page and the render checks draw this), and
          `aria-describedby` must name a node that is always there. */}
      <span id={id} className={styles.srOnly}>
        {children}
      </span>
      {open &&
        createPortal(
          <div
            ref={popup}
            aria-hidden
            className={styles.popover}
            style={place}
            onMouseEnter={clearTimer}
            onMouseLeave={hoverOut}
          >
            {children}
          </div>,
          document.body,
        )}
    </>
  )
}

/**
 * A paragraph inside an `InfoTip`.
 *
 * A `<span>` drawn as a block rather than a `<p>`, because the tip's text is rendered twice and
 * one copy sits inline — in a label, in a section's `<h3>` caption — where a `<p>` is invalid
 * nesting and React says so on every render.
 */
export function InfoPara({ children }: { children: ReactNode }): ReactElement {
  return <span className={styles.para}>{children}</span>
}
