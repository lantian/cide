/**
 * The pointer half of dragging rows between changelists. Every *rule* it applies lives in
 * `dragDrop.ts`; this file is the mechanism, and it is the part no check in this repo can run.
 *
 * # Pointer events, not HTML5 drag and drop
 *
 * The decision, and what lost:
 *
 * 1. **Tauri swallows HTML5 drags.** `dragDropEnabled` defaults to `true` on a webview and the
 *    native file-drop handler then eats `dragstart`/`dragover`/`drop` before the page sees
 *    them; turning it off is a line in `crates/cide-app/tauri.conf.json`, a file this feature
 *    does not own, and it would also disable dropping files onto the window everywhere else.
 *    A gesture that works in `pnpm dev` in a browser and does nothing in the shipped app is the
 *    worst of the two failure modes.
 * 2. **The drop indicator is ours either way.** `dragover` reports the element under the
 *    pointer, but the tree needs the *group* that element belongs to, which is a walk over the
 *    row list — so the useful half of the native API is not used, and what remains is a
 *    `dataTransfer` payload that only ever travels to this same component.
 * 3. **The app already drags this way.** `layout/Splitter.tsx`, `chrome/SidebarSplitter.tsx`
 *    and the editor minimap are all pointer capture plus a window listener. One drag mechanism
 *    in one app.
 *
 * What that costs, and what is paid: a 4px threshold before a press becomes a drag (so a click
 * still clicks), ⎋ to cancel, `pointercancel` handled, edge auto-scroll, and no cross-window
 * drop — the last of which is not a loss, since a changelist lives in one repository inside one
 * panel.
 *
 * # Hit testing
 *
 * `document.elementFromPoint` at every move, then `closest('[data-row-id]')` — the attribute
 * the context menu already reads rows back through. Not React `onPointerEnter` per row: the
 * pointer is captured during a drag, so enter/leave stop firing on the rows underneath, which
 * is precisely why capture is used (the moves keep arriving when the pointer leaves the panel).
 */
import { useCallback, useEffect, useRef, useState } from 'react'
import { dropOutcome, dropTarget, grab, type DragSet, type DropOutcome } from './dragDrop'
import type { Row } from './model'
import type { RepoId, StatusView } from './types'

/** Enough movement to mean "drag" rather than "click". Same 4px the splitters use. */
const THRESHOLD = 4

/** How close to an edge starts the auto-scroll, and how fast it runs. */
const EDGE = 24
const EDGE_STEP = 6

export interface ChangesDragState {
  drag: DragSet
  /** Viewport coordinates of the pointer — the ghost is `position: fixed`. */
  x: number
  y: number
  /** The group row id under the pointer, or `null`. What the tree outlines. */
  over: string | null
  outcome: DropOutcome
}

export interface UseChangesDragOptions {
  rows: readonly Row[]
  view: StatusView
  /**
   * The row selection resolved for the drag — `rowSelection.ts::carriedIds`.
   *
   * Not the ticks. A grab that starts on a selected row carries the whole selection; the
   * ticks are what a *commit* takes, and widening by them is what used to move a whole
   * changelist when the user dragged one file out of it. See the header of `dragDrop.ts`.
   */
  carried: ReadonlySet<string>
  /** The scrolling box, for the edge auto-scroll. */
  container: React.RefObject<HTMLDivElement | null>
  /**
   * Run the move. Absent — a fixture, or the server render — makes the tree inert rather than
   * pretending: a drag that cannot land is a drag that should not start.
   */
  onMove?: ((repo: RepoId, changelist: string, paths: string[]) => void) | undefined
}

export interface ChangesDrag {
  state: ChangesDragState | null
  /** Put on every row; it decides for itself whether that row can start a drag. */
  onPointerDown: (e: React.PointerEvent, row: Row) => void
  /**
   * Did the gesture that is ending right now turn into a drag?
   *
   * Read from `mouseup`, which is where this tree folds a group: a press that became a drag
   * must not also toggle the row it started on, or grabbing a directory would begin by
   * collapsing the directory. A function rather than a boolean field so it can be read after
   * the state has been cleared — `pointerup` clears the drag before `mouseup` fires.
   */
  dragged: () => boolean
}

export function useChangesDrag(options: UseChangesDragOptions): ChangesDrag {
  const { rows, view, carried, container, onMove } = options
  const [state, setState] = useState<ChangesDragState | null>(null)

  /*
   * The listeners run for the life of one gesture and must see the *current* tree: this panel
   * refreshes several times a second while an agent edits, and a handler closed over the rows
   * as they were at mousedown would drop onto a changelist that has since moved. Refs rather
   * than dependencies, because re-subscribing window listeners mid-drag loses the capture.
   */
  const live = useRef({ rows, view, carried, onMove })
  live.current = { rows, view, carried, onMove }
  /** Everything about the gesture in flight. `null` between gestures. */
  const gesture = useRef<{
    pointerId: number
    from: { x: number; y: number }
    row: Row
    drag: DragSet | null
    /**
     * The verdict as of the last move.
     *
     * Kept here and not read back out of React state at drop time. A state updater is called
     * twice under StrictMode and may be replayed at will, so it is the wrong place to fire a
     * command from — the same trap `useGitPanel::runConfirm` documents — and the ref is written
     * synchronously by the move that produced it, so there is no flush to race.
     */
    outcome: DropOutcome | null
    stop: (() => void) | null
  } | null>(null)
  /** Whether the *last* gesture became a drag. Cleared by the next press, not by the drop. */
  const wasDrag = useRef(false)

  const finish = useCallback((commit: boolean) => {
    const active = gesture.current
    gesture.current = null
    active?.stop?.()
    setState(null)
    const outcome = active?.outcome
    if (!commit || outcome === undefined || outcome === null || outcome.kind !== 'move') return
    live.current.onMove?.(outcome.repo, outcome.changelist, outcome.paths)
  }, [])

  const onPointerDown = useCallback(
    (e: React.PointerEvent, row: Row) => {
      /*
       * Left button only, and never from the checkbox (its own handler stops the event) or
       * from a modified press.
       *
       * A modified press is a *selection* gesture and nothing else: ctrl toggles a row into
       * the selection, shift extends the range to it (`rowSelection.ts`). Letting one start a
       * drag would mean ctrl-clicking a fourth file and moving the hand two pixels picked the
       * four up, which is the behaviour IDEA refuses too. The unmodified press that follows is
       * what drags them, and by then the press has already made the selection right.
       */
      if (e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) return
      if (gesture.current !== null) return
      // A new press: whatever the previous gesture was, this one is a click until it moves.
      wasDrag.current = false
      /*
       * Nothing to land on, so nothing is picked up.
       *
       * `onMove` is absent in a fixture and in the server render, and without this the drag
       * ran in full — rows dimmed, the ghost read `Move 4 files to “fixes”`, the target
       * ringed in the accent — and the drop did nothing whatever, because `finish` ends at an
       * optional call. That is the silent failure this feature exists to remove, dressed as
       * the feature. Read through `live` rather than closed over, so a panel that gains the
       * action between renders is not stuck inert until it remounts.
       */
      if (live.current.onMove === undefined) return
      if (grab(live.current.view, live.current.carried, row) === null) return

      const el = e.currentTarget
      if (!(el instanceof HTMLElement)) return

      const move = (ev: PointerEvent) => {
        const active = gesture.current
        if (active === null || ev.pointerId !== active.pointerId) return
        if (active.drag === null) {
          const far =
            Math.abs(ev.clientX - active.from.x) > THRESHOLD
            || Math.abs(ev.clientY - active.from.y) > THRESHOLD
          if (!far) return
          const started = grab(live.current.view, live.current.carried, active.row)
          if (started === null) {
            finish(false)
            return
          }
          active.drag = started
          wasDrag.current = true
          // Only once the press is a drag: capturing on mousedown would break the click, and
          // this tree opens diffs on a click.
          try {
            el.setPointerCapture(ev.pointerId)
          } catch {
            // A pointer that has already been released (a fast click) throws here. The drag
            // simply runs without capture, which is still correct — the listeners are on the
            // window.
          }
        }
        const drag = active.drag
        if (drag === null) return
        const under = document.elementFromPoint(ev.clientX, ev.clientY)
        const id =
          under instanceof Element
            ? (under.closest<HTMLElement>('[data-row-id]')?.dataset['rowId'] ?? null)
            : null
        const target = dropTarget(live.current.rows, id)
        const outcome = dropOutcome(drag, target)
        active.outcome = outcome
        setState({ drag, x: ev.clientX, y: ev.clientY, over: target?.id ?? null, outcome })
        edgeScroll(container.current, ev.clientY)
      }

      const up = (ev: PointerEvent) => {
        const active = gesture.current
        if (active === null || ev.pointerId !== active.pointerId) return
        finish(active.drag !== null)
      }
      const cancel = () => finish(false)
      const key = (ev: KeyboardEvent) => {
        if (ev.key !== 'Escape') return
        // Only while something is in flight, and it must not also close the panel or the
        // window behind it.
        if (gesture.current === null || gesture.current.drag === null) return
        ev.preventDefault()
        ev.stopPropagation()
        finish(false)
      }

      window.addEventListener('pointermove', move)
      window.addEventListener('pointerup', up)
      window.addEventListener('pointercancel', cancel)
      window.addEventListener('keydown', key, true)
      gesture.current = {
        pointerId: e.pointerId,
        from: { x: e.clientX, y: e.clientY },
        row,
        drag: null,
        outcome: null,
        stop: () => {
          window.removeEventListener('pointermove', move)
          window.removeEventListener('pointerup', up)
          window.removeEventListener('pointercancel', cancel)
          window.removeEventListener('keydown', key, true)
          if (el.hasPointerCapture(e.pointerId)) el.releasePointerCapture(e.pointerId)
        },
      }
    },
    [container, finish],
  )

  // A panel unmounted mid-drag (the sidebar switching to Files, the window closing) must not
  // leave four window listeners behind firing at a dead component.
  useEffect(() => () => finish(false), [finish])

  return { state, onPointerDown, dragged: () => wasDrag.current }
}

/**
 * Scroll the tree when the pointer is at its edge.
 *
 * Stepped per pointer *move* rather than on a timer: a timer keeps scrolling while the pointer
 * is still, which in a 420px panel overshoots the changelist the user stopped over. The cost is
 * that a motionless pointer at the edge does not scroll — a nudge is enough to continue, and
 * that is the trade every file manager makes the other way and gets wrong.
 */
function edgeScroll(box: HTMLElement | null, y: number): void {
  if (box === null) return
  const rect = box.getBoundingClientRect()
  if (y < rect.top + EDGE) box.scrollTop -= EDGE_STEP
  else if (y > rect.bottom - EDGE) box.scrollTop += EDGE_STEP
}
