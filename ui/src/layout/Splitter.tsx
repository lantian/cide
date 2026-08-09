/**
 * One divider between the two children of a split.
 *
 * The single most important performance decision in the pane grid lives here: **a drag
 * writes `gridTemplateColumns` / `gridTemplateRows` straight to the parent grid's DOM node
 * and commits to Rust only on `pointerup`.** Routing pointer moves through React state
 * would re-render the whole workspace on every move — the file tree, the tab strip and
 * every sibling pane — and a re-render next to four live terminals is not free: each
 * `PaneSlot` resize fires `fit()`, which reflows xterm and sends a SIGWINCH to a real
 * child process. Dragging a divider would go from smooth to visibly stepped, and the
 * children would be resized dozens of times for one gesture instead of once.
 *
 * The consequence is that between `pointerdown` and the snapshot coming back, the DOM is
 * ahead of the model. `SplitTree` re-asserts the template from props on every render it
 * makes while no drag is in flight, which is what puts the two back in agreement.
 */
import { useCallback, useEffect, useRef, useState } from 'react'
import type {
  CSSProperties,
  KeyboardEvent as ReactKeyboardEvent,
  PointerEvent as ReactPointerEvent,
  RefObject,
} from 'react'
// Types only, fully erased at build time: this component must be renderable from a fixture
// with no Rust behind it, which is why nothing here imports the `@/ipc` client itself.
import type { Axis, SplitId } from '@/ipc/generated'
import styles from './SplitTree.module.css'

/** The domain's clamp, mirrored from `cide-core`'s `MIN_RATIO` / `MAX_RATIO`. */
export const MIN_RATIO = 0.1
export const MAX_RATIO = 0.9

/** Arrow-key nudge, as a fraction of the split. */
/**
 * How long a keyboard nudge waits before it reaches the domain.
 *
 * Long enough that a held key coalesces into a handful of commits rather than one per
 * repeat, short enough that a single tap still feels immediate. Flushed on keyup, blur and
 * unmount, so no movement is ever lost to the timer.
 */
const KEY_COMMIT_DELAY = 120

const KEY_STEP = 0.02

export function clampRatio(ratio: number): number {
  return Math.min(MAX_RATIO, Math.max(MIN_RATIO, ratio))
}

/**
 * The track list for a split: the two children with the splitter's fixed track between them.
 *
 * Fractions rather than the mock's literal `1.35fr … 1fr` — the same used track sizes, since
 * `fr` is relative and both children resolve against the same free space.
 */
export function splitTemplate(ratio: number): string {
  return `${ratio}fr var(--w-splitter) ${1 - ratio}fr`
}

/** Write the template for `axis` onto a grid element, bypassing React entirely. */
export function applyTemplate(grid: HTMLElement, axis: Axis, ratio: number): void {
  const template = splitTemplate(ratio)
  if (axis === 'row') grid.style.gridTemplateColumns = template
  else grid.style.gridTemplateRows = template
}

export interface SplitterProps {
  split: SplitId
  /** The split's axis: `row` children sit left/right, so the divider is vertical. */
  axis: Axis
  ratio: number
  /** The grid being divided. The ratio is read off *its* box, never the splitter's. */
  gridRef: RefObject<HTMLDivElement | null>
  /** Hidden rather than unmounted while a sibling is maximized. */
  hidden?: boolean | undefined
  /** Lets the parent suspend its template re-assert for the duration of a drag. */
  onDragActive?: ((active: boolean) => void) | undefined
  onCommit?: ((split: SplitId, ratio: number) => void) | undefined
}

export function Splitter({
  split,
  axis,
  ratio,
  gridRef,
  hidden = false,
  onDragActive,
  onCommit,
}: SplitterProps) {
  const vertical = axis === 'row'
  const [active, setActive] = useState(false)
  const dragging = useRef(false)
  // The ratio the DOM is currently showing. Diverges from the prop during a drag, and
  // between a keyboard nudge and the snapshot that answers it — without it, key repeat
  // would compute every step from the same stale prop and the divider would move once.
  const live = useRef(ratio)
  /** The separator element, for imperative `aria-valuenow` during a gesture. */
  const selfRef = useRef<HTMLDivElement>(null)
  /** Pending keyboard commit, so a held arrow key does not commit per repeat. */
  const commitTimer = useRef<number | null>(null)
  useEffect(
    () => () => {
      if (commitTimer.current !== null) window.clearTimeout(commitTimer.current)
    },
    [],
  )

  useEffect(() => {
    if (!dragging.current) live.current = ratio
  }, [ratio])

  const stop = useCallback(
    (commit: boolean) => {
      if (!dragging.current) return
      dragging.current = false
      setActive(false)
      onDragActive?.(false)
      document.body.style.cursor = ''
      document.body.style.userSelect = ''
      if (commit) onCommit?.(split, live.current)
    },
    [onCommit, onDragActive, split],
  )

  // A splitter can be unmounted mid-drag by a snapshot that removes its split. Without this
  // the body would keep `user-select: none` and a resize cursor for the rest of the session.
  useEffect(
    () => () => {
      if (dragging.current) {
        document.body.style.cursor = ''
        document.body.style.userSelect = ''
      }
    },
    [],
  )

  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || !gridRef.current) return
    // Stops the gesture from starting a text selection in the pane it began over.
    e.preventDefault()
    e.currentTarget.setPointerCapture(e.pointerId)
    dragging.current = true
    live.current = ratio
    setActive(true)
    onDragActive?.(true)
    // On the body, not the splitter: the pointer spends the whole drag over the panes, and
    // without these the cursor flickers to a text caret every time it crosses one.
    document.body.style.cursor = vertical ? 'col-resize' : 'row-resize'
    document.body.style.userSelect = 'none'
  }

  const onPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    const grid = gridRef.current
    if (!dragging.current || !grid) return
    const box = grid.getBoundingClientRect()
    const raw = vertical ? (e.clientX - box.left) / box.width : (e.clientY - box.top) / box.height
    live.current = clampRatio(raw)
    applyTemplate(grid, axis, live.current)
  }

  /**
   * Move the divider now, tell the domain shortly.
   *
   * A held arrow key repeats at roughly 30 Hz, and committing on each repeat is the same
   * re-render storm the pointer path exists to avoid, arriving through the keyboard: every
   * commit lands a new tree in the store, re-renders the tab, resizes every pane and fires
   * `fit()` plus a `session.resize` on each live PTY. The DOM write stays immediate so the
   * divider tracks the key; only the commit waits.
   *
   * `aria-valuenow` is set imperatively alongside it. Reading it from the model prop would
   * make a screen reader announce the old position for as long as the commit takes, and in
   * a fixture with no `onCommit` wired it would never change at all — but routing it
   * through state would re-render mid-gesture, which is the thing being avoided.
   */
  const moveTo = (next: number) => {
    const grid = gridRef.current
    live.current = clampRatio(next)
    if (grid) applyTemplate(grid, axis, live.current)
    selfRef.current?.setAttribute('aria-valuenow', String(Math.round(live.current * 100)))

    if (commitTimer.current !== null) window.clearTimeout(commitTimer.current)
    commitTimer.current = window.setTimeout(() => {
      commitTimer.current = null
      onCommit?.(split, live.current)
    }, KEY_COMMIT_DELAY)
  }

  /** Send a pending keyboard commit immediately, so none is ever dropped. */
  const flushCommit = () => {
    if (commitTimer.current === null) return
    window.clearTimeout(commitTimer.current)
    commitTimer.current = null
    onCommit?.(split, live.current)
  }

  const onKeyDown = (e: ReactKeyboardEvent<HTMLDivElement>) => {
    const back = vertical ? 'ArrowLeft' : 'ArrowUp'
    const forward = vertical ? 'ArrowRight' : 'ArrowDown'
    if (e.key === back) moveTo(live.current - KEY_STEP)
    else if (e.key === forward) moveTo(live.current + KEY_STEP)
    else if (e.key === 'Home') moveTo(MIN_RATIO)
    else if (e.key === 'End') moveTo(MAX_RATIO)
    else return
    e.preventDefault()
  }

  // The middle track, stated rather than auto-placed: see `place` in SplitTree.tsx for what
  // auto-placement does to this element once a sibling branch spans the whole grid.
  const placement: CSSProperties = vertical
    ? { gridColumn: '2', gridRow: '1' }
    : { gridRow: '2', gridColumn: '1' }

  const className = [
    styles.splitter,
    vertical ? styles.splitterVertical : styles.splitterHorizontal,
    active ? styles.splitterActive : '',
  ]
    .filter(Boolean)
    .join(' ')

  return (
    <div
      role="separator"
      aria-orientation={vertical ? 'vertical' : 'horizontal'}
      aria-label={vertical ? 'Resize panes horizontally' : 'Resize panes vertically'}
      aria-valuenow={Math.round(ratio * 100)}
      aria-valuemin={MIN_RATIO * 100}
      aria-valuemax={MAX_RATIO * 100}
      tabIndex={0}
      ref={selfRef}
      onKeyUp={flushCommit}
      onBlur={flushCommit}
      data-audit="splitter"
      data-split={split}
      className={className}
      style={hidden ? { ...placement, visibility: 'hidden' } : placement}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={() => stop(true)}
      onPointerCancel={() => stop(false)}
      // Safety net: fires after `pointerup` too, where `stop` has already run and returns.
      onLostPointerCapture={() => stop(false)}
      onKeyDown={onKeyDown}
    />
  )
}
