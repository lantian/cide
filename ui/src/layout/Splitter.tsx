/**
 * One divider between two adjacent members of a chain.
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
 *
 * Two things were still paid per *pointer event* rather than per frame, and WebKitGTK does not
 * coalesce `pointermove` to frames — a 125 Hz mouse reports 125 times a second. So:
 *
 * * The grid's box is measured **once, at `pointerdown`**, rather than on every move. The read
 *   used to sit immediately after the previous move's style write, which is a forced synchronous
 *   layout of the whole pane subtree — terminals included — at the mouse's report rate. The only
 *   thing that can invalidate it mid-drag is the window itself changing size, which a tiling
 *   window manager can do under a held button, so a `resize` listener lives for the drag.
 * * The style write is coalesced onto one `requestAnimationFrame`. The arithmetic stays
 *   synchronous, so `live.current` is always the pointer's real answer and `pointerup` can
 *   commit without waiting for a frame; only the DOM write waits.
 *
 * And the drag is bracketed by `resizeGesture`, which is what stops every terminal in every tab
 * refitting on every frame of it. See that module — the decision that the terminal grid does not
 * follow the drag is written down there, not here.
 *
 * Since M11 the grid this divides holds a whole *chain* — `2n - 1` tracks, one per member
 * with a divider between each pair — so the arithmetic below works in shares rather than in
 * one ratio. The property that buys is exact: `applyDrag` copies every fraction outside the
 * dragged pair bit for bit, so a divider in one row provably cannot move another row's
 * tiles. That used to depend on the tree happening to be shaped as columns.
 */
import { useEffect, useRef, useState } from 'react'
import type {
  CSSProperties,
  KeyboardEvent as ReactKeyboardEvent,
  PointerEvent as ReactPointerEvent,
  RefObject,
} from 'react'
// Types only, fully erased at build time: this component must be renderable from a fixture
// with no Rust behind it, which is why nothing here imports the `@/ipc` client itself.
import type { Axis, SplitId } from '@/ipc/generated'
import { lockBodyForDrag, unlockBodyAfterDrag } from '@/chrome/dragLock'
import { beginResizeGesture, endResizeGesture } from './resizeGesture'
import styles from './SplitTree.module.css'

/**
 * A member's floor, mirrored from `cide-core`'s `MIN_TILE`.
 *
 * Still the same two literals the file has always carried: `cide-core` sets
 * `MIN_TILE = MIN_RATIO = 0.1` precisely so that a share floor and the stored ratio band are
 * the same number, and neither side has to compute a function of the chain's arity.
 */
export const MIN_TILE = 0.1
export const MAX_TILE = 1 - MIN_TILE

/** The stored band, unchanged. Kept for the `aria-value*` on a two-member chain. */
export const MIN_RATIO = MIN_TILE
export const MAX_RATIO = MAX_TILE

/**
 * How long a keyboard nudge waits before it reaches the domain.
 *
 * Long enough that a held key coalesces into a handful of commits rather than one per
 * repeat, short enough that a single tap still feels immediate. Flushed on keyup, blur and
 * unmount, so no movement is ever lost to the timer.
 */
const KEY_COMMIT_DELAY = 120

/** Arrow-key nudge, as a fraction of the pair. */
const KEY_STEP = 0.02

/**
 * The track list for a chain: every member's `fr`, with the splitter's fixed track between
 * each adjacent pair.
 *
 * Fractions rather than the mock's literal `1.35fr … 1fr` — the same used track sizes, since
 * `fr` is relative and every member resolves against the same free space.
 */
export function trackTemplate(f: number[]): string {
  return f.map((w) => `${w}fr`).join(' var(--w-splitter) ')
}

/**
 * Write the template for `axis` onto a grid element, bypassing React entirely.
 *
 * Skips the write when the element already carries the exact string. `ChainNode` calls this
 * from a no-dependency `useLayoutEffect` — deliberately, to repair a drag the domain clamped
 * — so in the common case the value here is the one React just committed via the `style`
 * prop, and re-writing it dirties style on every chain of every mounted tab in the same
 * commit where `TabStrip` then reads layout. The drag path always writes a genuinely new
 * string, so drags never hit the skip.
 */
export function applyTracks(grid: HTMLElement, axis: Axis, f: number[]): void {
  const template = trackTemplate(f)
  const prop = axis === 'row' ? 'gridTemplateColumns' : 'gridTemplateRows'
  if (grid.style[prop] === template) return
  grid.style[prop] = template
}

/** The two members divider `k` separates, and how much of the chain they own between them. */
export function pairOf(f: number[], k: number): { p: number; lo: number; hi: number } {
  const p = (f[k] ?? 0) + (f[k + 1] ?? 0)
  const lo = MIN_TILE / p
  // On a legacy chain a pair can be narrower than two tiles, and the two ends cross over.
  // Halving is the one answer legal from both directions — the same rule `cide-core` uses.
  return lo <= 1 - lo ? { p, lo, hi: 1 - lo } : { p, lo: 0.5, hi: 0.5 }
}

export function clampPair(f: number[], k: number, t: number): number {
  const { lo, hi } = pairOf(f, k)
  return Math.min(hi, Math.max(lo, t))
}

/**
 * The chain's fractions with divider `k` moved to pair share `t`.
 *
 * The pair's own budget `p` is preserved, so every other member is *copied*, never
 * recomputed — which is why the invariance is exact rather than within a pixel.
 */
export function applyDrag(f: number[], k: number, t: number): number[] {
  const { p } = pairOf(f, k)
  const next = f.slice()
  next[k] = p * t
  next[k + 1] = p * (1 - t)
  return next
}

export interface SplitterProps {
  split: SplitId
  /** The chain's axis: `row` members sit left/right, so the divider is vertical. */
  axis: Axis
  /** Every member's share of the chain, in order. */
  fractions: number[]
  /** This divider's in-order index: it separates members `index` and `index + 1`. */
  index: number
  /** The grid being divided. Sizes are read off *its* box, never the splitter's. */
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
  fractions,
  index,
  gridRef,
  hidden = false,
  onDragActive,
  onCommit,
}: SplitterProps) {
  const vertical = axis === 'row'
  const { p, lo, hi } = pairOf(fractions, index)
  const share = p > 0 ? (fractions[index] ?? 0) / p : 0.5

  const [active, setActive] = useState(false)
  const dragging = useRef(false)
  // The pair share the DOM is currently showing. Diverges from the prop during a drag, and
  // between a keyboard nudge and the snapshot that answers it — without it, key repeat
  // would compute every step from the same stale prop and the divider would move once.
  const live = useRef(share)
  /** The separator element, for imperative `aria-valuenow` during a gesture. */
  const selfRef = useRef<HTMLDivElement>(null)
  /**
   * The splitter track's own width, measured once at `pointerdown`.
   *
   * Measured rather than parsed out of `getComputedStyle(--w-splitter)`, and rather than
   * derived from the neighbours' rects: computing everything from `fractions` plus this one
   * number keeps the handler self-contained and immune to a hidden neighbour reporting a
   * zero-sized rect while a pane is maximized.
   */
  const gutter = useRef(0)
  /**
   * The grid's box, measured once at `pointerdown`.
   *
   * Measuring it per move is a forced synchronous layout of everything in the grid — including
   * every terminal's rows — immediately after the previous move wrote a new template, at the
   * mouse's report rate. Nothing a divider drag itself does can change this box: it redistributes
   * tracks *inside* it. The one thing that can is the window changing size under a held button,
   * which a tiling window manager will do, so [`onPointerDown`] watches for that and re-measures.
   */
  const box = useRef({ left: 0, top: 0, width: 0, height: 0 })
  /** Removes the drag-lifetime `resize` listener that keeps [`box`] honest. */
  const unwatch = useRef<(() => void) | null>(null)
  /** The pending style write, so a burst of moves paints once. */
  const frame = useRef<number | null>(null)
  /**
   * True while a run of arrow-key nudges is open.
   *
   * A held arrow repeats at roughly 30 Hz and each repeat resizes the panes, so key repeat is a
   * resize gesture in every sense that matters here. Opened on the first nudge and closed by
   * `flushCommit`, which already runs on keyup, blur and unmount.
   */
  const keying = useRef(false)
  /** Pending keyboard commit, so a held arrow key does not commit per repeat. */
  const commitTimer = useRef<number | null>(null)
  useEffect(
    () => () => {
      if (commitTimer.current !== null) window.clearTimeout(commitTimer.current)
    },
    [],
  )

  useEffect(() => {
    if (!dragging.current) live.current = share
  }, [share])

  /**
   * Write the divider's current position to the DOM.
   *
   * `aria-valuenow` rides along imperatively: reading it from the model prop would make a screen
   * reader announce the old position for as long as the commit takes, and in a fixture with no
   * `onCommit` wired it would never change at all — but routing it through state would re-render
   * mid-gesture, which is the thing being avoided.
   */
  const paint = () => {
    // The aria first, and unconditionally: the tracks need a grid and the announcement does not,
    // and a fixture rendering this splitter without one must still report where it moved to.
    selfRef.current?.setAttribute('aria-valuenow', String(Math.round(live.current * 100)))
    const grid = gridRef.current
    if (!grid) return
    applyTracks(grid, axis, applyDrag(fractions, index, live.current))
  }

  /** Paint at most once per frame, however many pointer events or key repeats arrive. */
  const schedulePaint = () => {
    if (frame.current !== null) return
    frame.current = requestAnimationFrame(() => {
      frame.current = null
      paint()
    })
  }

  /** Drop a scheduled paint. The caller is about to paint the final position itself. */
  const cancelPaint = () => {
    if (frame.current === null) return
    cancelAnimationFrame(frame.current)
    frame.current = null
  }

  /** Undo everything `pointerdown` set up. Safe to call twice; the second call does nothing. */
  const release = () => {
    unwatch.current?.()
    unwatch.current = null
    unlockBodyAfterDrag()
    endResizeGesture()
  }

  const stop = (commit: boolean) => {
    if (!dragging.current) return
    dragging.current = false
    setActive(false)
    onDragActive?.(false)
    // The last pointer event may not have been painted yet, and the gesture is over: paint the
    // position being committed rather than letting a frame land after the model has moved.
    cancelPaint()
    paint()
    release()
    if (commit) onCommit?.(split, live.current)
  }

  // A splitter can be unmounted mid-drag by a snapshot that removes its split. Without this
  // the body would keep the selection lock and a resize cursor for the rest of the session —
  // and the resize gesture would stay open, which is worse: every terminal in the window would
  // stop refitting until `resizeGesture`'s watchdog noticed.
  useEffect(
    () => () => {
      cancelPaint()
      if (dragging.current) {
        dragging.current = false
        release()
      }
      if (keying.current) {
        keying.current = false
        endResizeGesture()
      }
    },
    // Intentionally empty: this is the unmount path, and the closure only touches refs.
    [],
  )

  /** Re-read the grid's box into [`box`]. */
  const measureGrid = () => {
    const grid = gridRef.current
    if (!grid) return
    const rect = grid.getBoundingClientRect()
    box.current = { left: rect.left, top: rect.top, width: rect.width, height: rect.height }
  }

  const onPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (e.button !== 0 || !gridRef.current) return
    // Stops the gesture from starting a text selection in the pane it began over.
    e.preventDefault()
    e.currentTarget.setPointerCapture(e.pointerId)
    const self = e.currentTarget.getBoundingClientRect()
    gutter.current = vertical ? self.width : self.height
    measureGrid()
    // The one thing that can invalidate the measurement above without this splitter knowing:
    // a window manager resizing the window while the button is held. Rare, and the failure is
    // a divider that lands somewhere the pointer is not, so it costs one listener to rule out.
    const onWindowResize = () => measureGrid()
    window.addEventListener('resize', onWindowResize)
    unwatch.current = () => window.removeEventListener('resize', onWindowResize)
    dragging.current = true
    live.current = share
    setActive(true)
    onDragActive?.(true)
    // Before the first move, so nothing downstream refits on the frames this drag is about to
    // produce. Paired in `stop` and in the unmount effect.
    beginResizeGesture()
    // On the body, not the splitter: the pointer spends the whole drag over the panes, and
    // without this the cursor flickers to a text caret every time it crosses one. Through
    // `dragLock` because the selection half of it cannot be written as `style.userSelect` in
    // this engine and silently appears to work — see that module.
    lockBodyForDrag(vertical ? 'col-resize' : 'row-resize')
  }

  const onPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (!dragging.current) return
    const { left, top, width, height } = box.current
    const extent = vertical ? width : height
    // The gutters are fixed tracks, so the `fr` shares divide only what is left of the box.
    const usable = extent - (fractions.length - 1) * gutter.current
    if (usable <= 0 || p <= 0) return
    // Where this pair starts inside the grid: every earlier member's share of the usable
    // space, plus the fixed gutters already crossed.
    const start = fractions.slice(0, index).reduce((s, w) => s + w, 0) * usable
      + index * gutter.current
    const at = (vertical ? e.clientX - left : e.clientY - top) - start
    // Computed now, painted on the next frame: `stop` commits `live.current`, so it must be the
    // pointer's real answer at all times and not something a dropped frame could round off.
    live.current = clampPair(fractions, index, at / (p * usable))
    schedulePaint()
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
   * The run of repeats is bracketed as one resize gesture, for the same reason the pointer drag
   * is: each repeat changes every pane's box, and a refit per repeat is the same storm arriving
   * through the keyboard. `flushCommit` — already wired to keyup, blur and unmount — closes it.
   */
  const moveTo = (next: number) => {
    if (!keying.current) {
      keying.current = true
      beginResizeGesture()
    }
    live.current = clampPair(fractions, index, next)
    // The DOM write stays on the next frame rather than inline: 30 Hz of key repeat is still
    // more writes than there are frames to show them.
    schedulePaint()

    if (commitTimer.current !== null) window.clearTimeout(commitTimer.current)
    commitTimer.current = window.setTimeout(() => {
      commitTimer.current = null
      onCommit?.(split, live.current)
    }, KEY_COMMIT_DELAY)
  }

  /** Send a pending keyboard commit immediately, so none is ever dropped. */
  const flushCommit = () => {
    if (keying.current) {
      keying.current = false
      cancelPaint()
      paint()
      endResizeGesture()
    }
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
    else if (e.key === 'Home') moveTo(lo)
    else if (e.key === 'End') moveTo(hi)
    else return
    e.preventDefault()
  }

  // The divider's own track, stated rather than auto-placed: see `place` in SplitTree.tsx
  // for what auto-placement does to this element once a member spans the whole grid.
  const track = String(2 * index + 2)
  const placement: CSSProperties = vertical
    ? { gridColumn: track, gridRow: '1' }
    : { gridRow: track, gridColumn: '1' }

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
      aria-valuenow={Math.round(share * 100)}
      aria-valuemin={Math.round(lo * 100)}
      aria-valuemax={Math.round(hi * 100)}
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
