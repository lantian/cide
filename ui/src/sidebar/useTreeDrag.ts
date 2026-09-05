/**
 * The pointer half of dragging file-tree rows into a folder. Every *rule* it applies lives in
 * `treeDrag.ts`; this file is the mechanism, and it is the part no check in this repo can run.
 *
 * # Pointer events, not HTML5 drag and drop
 *
 * The same decision `GitPanel/useChangesDrag.ts` records, re-checked against this tree rather
 * than assumed:
 *
 * 1. **Tauri's native drag-drop handler is on.** `crates/cide-app/src/windows.rs` builds every
 *    window with `WebviewWindowBuilder::new(…)` and never calls `disable_drag_drop_handler()`,
 *    and `tauri.conf.json` has an empty `app.windows` array, so there is no config route either.
 *    Turning it off is a global change owned by nobody in this feature and it would kill OS
 *    file-drop onto the window everywhere. On WebKitGTK the swallowing is engine-dependent rather
 *    than the documented Windows certainty — which is an argument *for* pointer events, not
 *    against: a gesture that works in `pnpm dev` in a browser and does nothing in the shipped app
 *    is the worse failure, and it is the one this repo has shipped repeatedly.
 * 2. **The useful half of the native API is unused anyway.** `dragover` reports the element under
 *    the pointer; this tree needs the *folder* that element resolves to (a file row means its
 *    parent), which is a lookup either way. What remains is a `dataTransfer` payload that only
 *    ever travels to the same component.
 * 3. **One drag mechanism in one app.** `layout/Splitter.tsx`, `chrome/SidebarSplitter.tsx`, the
 *    editor minimap and `useChangesDrag.ts` are all pointer capture plus window listeners.
 *
 * Cost, accepted and already the stated non-goal of the clipboard (`clipboardModel.ts`'s header):
 * no drag *into* cide from Dolphin or Nautilus, and none *out*. Those need the desktop's own
 * selection flavours, which a webview cannot serve; this feature does not make that worse.
 * (Since M39 one surface does take a desktop drop — the task card, for attachments — and it
 * can because it wants *paths*, which Tauri's own drag-drop event hands the page after it has
 * swallowed the HTML5 one. `sidebar/TasksPanel/fileDrop.ts` has the argument; the tree still
 * cannot, because a drop here would mean a copy and this hook is a move.)
 *
 * # What is deliberately **not** copied from the git panel's hook
 *
 * `ChangesTree` calls `preventDefault()` on the row's mousedown to stop the engine starting a
 * text selection. Doing that here would break the tree: rows are not focusable and the scroller
 * is the whole panel's single tab stop (`role="tree" tabIndex={0}`), so a press whose default is
 * prevented never focuses it and every subsequent arrow key goes nowhere. Text selection is
 * already refused by the prefixed `user-select: none` on the body — see `styles/tokens.css` and
 * `check:css-prefix` — so there is nothing left for the `preventDefault` to buy.
 *
 * # Hit testing, and why the target is held as a path
 *
 * `document.elementFromPoint` at every move, then `closest('[data-row-path]')` — the attribute
 * the context menu already reads rows back through. Not React's `onPointerEnter` per row: the
 * pointer is captured during a drag, so enter/leave stop firing on the rows underneath, which is
 * precisely why capture is used (the moves keep arriving when the pointer leaves the panel).
 *
 * The path is then resolved to a live row through the store rather than being remembered. This
 * tree is **windowed and virtualized** and a spring-loaded expand re-flattens it, renumbering
 * every row below the folder that opened — so an index held across two frames names a different
 * file, which is the same hazard `FileTree.tsx::apply` guards for a Ctrl-click on a twisty.
 */
import { useCallback, useEffect, useRef, useState } from 'react'
import {
  dropOutcome,
  grab,
  springTarget,
  type DragRow,
  type TreeDragSet,
  type TreeDropOutcome,
} from './treeDrag'

/** Enough movement to mean "drag" rather than "click". The same 4px the splitters use. */
const THRESHOLD = 4

/** How close to an edge starts the auto-scroll, and how far each move steps it. */
const EDGE = 24
const EDGE_STEP = 6

/**
 * How long the pointer rests on a closed folder before it opens.
 *
 * 600 ms is IDEA's and the GNOME/KDE file managers' figure, and the number matters in both
 * directions: shorter and every folder the pointer crosses on the way somewhere flaps open,
 * which re-flattens the tree under a moving target; longer and people conclude it does not work
 * and give up on the drag.
 */
const SPRING_MS = 600

export interface TreeDragState {
  drag: TreeDragSet
  /** Viewport coordinates of the pointer — the ghost is `position: fixed`. */
  x: number
  y: number
  outcome: TreeDropOutcome
}

export interface UseTreeDragOptions {
  /** The selection, as paths. A grab on a selected row carries all of it. */
  carried: ReadonlySet<string>
  roots: readonly string[]
  /** Where cide may write — `fs_writable_roots`, roots plus the scratch drawer. */
  writable: readonly string[]
  /** The scrolling box, for the edge auto-scroll. */
  container: React.RefObject<HTMLDivElement | null>
  /** The live row for a path, or `null` — `treeStore.indexOf` then `rowAt`. */
  rowFor: (path: string) => DragRow | null
  /** Unfold a folder the pointer has rested on. Absent simply means no spring loading. */
  onSpring?: ((path: string) => void) | undefined
  /**
   * Run the move. Absent — a fixture, or a server render — makes the tree inert rather than
   * pretending: a drag that cannot land is a drag that should not start.
   */
  onMove?: ((sources: readonly string[], destDir: string) => void) | undefined
}

export interface TreeDrag {
  state: TreeDragState | null
  /** Put on every row; it decides for itself whether that row can start a drag. */
  onPointerDown: (e: React.PointerEvent, row: DragRow) => void
  /**
   * Did the gesture that is ending right now turn into a drag?
   *
   * Read from `mouseup`, where the deferred collapse of a multi-row selection happens: a press
   * that became a drag must not also collapse the selection it just moved. A function rather than
   * a field so it can be read after the state has been cleared — `pointerup` clears the drag
   * before `mouseup` fires.
   */
  dragged: () => boolean
}

export function useTreeDrag(options: UseTreeDragOptions): TreeDrag {
  const { carried, roots, writable, container, rowFor, onSpring, onMove } = options
  const [state, setState] = useState<TreeDragState | null>(null)

  /*
   * The listeners run for the life of one gesture and must see the *current* tree: a watcher
   * burst repaints this panel while an agent edits, and a handler closed over the selection and
   * the roots as they were at mousedown would drop into a folder that has since moved. Refs
   * rather than dependencies, because re-subscribing window listeners mid-drag loses the capture.
   */
  const live = useRef({ carried, roots, writable, rowFor, onSpring, onMove })
  live.current = { carried, roots, writable, rowFor, onSpring, onMove }

  /** Everything about the gesture in flight. `null` between gestures. */
  const gesture = useRef<{
    pointerId: number
    from: { x: number; y: number }
    row: DragRow
    drag: TreeDragSet | null
    /**
     * The verdict as of the last move.
     *
     * Kept here and not read back out of React state at drop time. A state updater is called
     * twice under StrictMode and may be replayed at will, so it is the wrong place to fire a
     * file move from — the trap `useGitPanel::runConfirm` documents — and this ref is written
     * synchronously by the move that produced it, so there is no flush to race.
     */
    outcome: TreeDropOutcome | null
    /** The folder the spring timer is counting down on, and the timer itself. */
    spring: { path: string; timer: ReturnType<typeof setTimeout> } | null
    stop: (() => void) | null
  } | null>(null)

  /** Whether the *last* gesture became a drag. Cleared by the next press, not by the drop. */
  const wasDrag = useRef(false)

  const finish = useCallback((commit: boolean) => {
    const active = gesture.current
    gesture.current = null
    const spring = active?.spring ?? null
    if (spring !== null) clearTimeout(spring.timer)
    active?.stop?.()
    setState(null)
    const outcome = active?.outcome
    if (!commit || outcome === undefined || outcome === null || outcome.kind !== 'move') return
    live.current.onMove?.(outcome.paths, outcome.destDir)
  }, [])

  /**
   * Start, restart or cancel the spring timer for the row under the pointer.
   *
   * Keyed on the **path**, so crossing three folders on the way to a fourth starts one countdown
   * per folder and only the one the pointer settles on ever fires. The timer re-reads the row
   * when it goes off rather than closing over the one it was started with: 600 ms is long enough
   * for a watcher burst to have replaced it, and expanding a stale row would be a `fs_expand` on
   * a path that no longer has one.
   */
  const springOn = useCallback((path: string | null) => {
    const active = gesture.current
    if (active === null) return
    if (active.spring?.path === path) return
    if (active.spring !== null) clearTimeout(active.spring.timer)
    if (path === null) {
      active.spring = null
      return
    }
    const timer = setTimeout(() => {
      const current = gesture.current
      if (current === null || current.spring?.path !== path) return
      current.spring = null
      // Asked again through the store, and gated on the same rule that started the timer: a
      // folder that has been expanded by something else in the meantime must not be folded.
      const row = live.current.rowFor(path)
      if (row !== null && row.kind === 'dir' && !row.expanded && row.hasChildren) {
        live.current.onSpring?.(path)
      }
    }, SPRING_MS)
    active.spring = { path, timer }
  }, [])

  const onPointerDown = useCallback(
    (e: React.PointerEvent, row: DragRow) => {
      /*
       * Left button only, and never from a modified press.
       *
       * A modified press is a *selection* gesture and nothing else: Ctrl toggles a row into the
       * selection, Shift extends the band to it (`treeSelection.ts`). Letting one start a drag
       * would mean Ctrl-clicking a fourth file and moving the hand two pixels picked all four up,
       * which is the behaviour IDEA refuses too. The unmodified press that follows is what drags
       * them, and by then the press has already made the selection right.
       */
      if (e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) return
      if (gesture.current !== null) return
      // A new press: whatever the previous gesture was, this one is a click until it moves.
      wasDrag.current = false
      /*
       * Nothing to land on, so nothing is picked up.
       *
       * `onMove` is absent in a fixture and in a server render, and without this the drag would
       * run in full — rows dimmed, the ghost reading `Move 4 items to “sidebar”`, the folder
       * ringed in the accent — and the drop would do nothing whatever, because `finish` ends at
       * an optional call. That is the silent failure this feature exists to remove, dressed as
       * the feature. Read through `live` rather than closed over, so a panel that gains the
       * action between renders is not stuck inert until it remounts.
       */
      if (live.current.onMove === undefined) return
      if (grab(live.current.carried, row, live.current.roots, live.current.writable) === null) return

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
          // Grabbed again at the threshold rather than at the press: the selection may have been
          // rewritten by the press itself, and this is the set the user can see highlighted.
          const started = grab(
            live.current.carried,
            active.row,
            live.current.roots,
            live.current.writable,
          )
          if (started === null) {
            finish(false)
            return
          }
          active.drag = started
          wasDrag.current = true
          // Only once the press is a drag: capturing on mousedown would break the click, and
          // this tree opens files on a double-click.
          try {
            el.setPointerCapture(ev.pointerId)
          } catch {
            // A pointer already released (a very fast click) throws here. The drag simply runs
            // without capture, which is still correct — the listeners are on the window.
          }
        }
        const drag = active.drag
        if (drag === null) return
        const under = document.elementFromPoint(ev.clientX, ev.clientY)
        const path =
          under instanceof Element
            ? (under.closest<HTMLElement>('[data-row-path]')?.dataset['rowPath'] ?? null)
            : null
        const target = path === null ? null : live.current.rowFor(path)
        const outcome = dropOutcome(drag, target, live.current.roots, live.current.writable)
        active.outcome = outcome
        setState({ drag, x: ev.clientX, y: ev.clientY, outcome })
        springOn(springTarget(target, outcome))
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
        // Only while something is in flight, and it must not also cancel the tree's pending cut
        // or close whatever is behind the panel — hence the capture-phase listener and the stop.
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
        spring: null,
        stop: () => {
          window.removeEventListener('pointermove', move)
          window.removeEventListener('pointerup', up)
          window.removeEventListener('pointercancel', cancel)
          window.removeEventListener('keydown', key, true)
          if (el.hasPointerCapture(e.pointerId)) el.releasePointerCapture(e.pointerId)
        },
      }
    },
    [container, finish, springOn],
  )

  // A panel unmounted mid-drag — the activity rail switching to Git, the window closing — must
  // not leave four window listeners and a spring timer behind, firing at a dead component.
  useEffect(() => () => finish(false), [finish])

  return { state, onPointerDown, dragged: () => wasDrag.current }
}

/**
 * Scroll the tree when the pointer is at its edge.
 *
 * Stepped per pointer *move* rather than on a timer: a timer keeps scrolling while the pointer is
 * still, which overshoots the folder the user stopped over. The cost is that a motionless pointer
 * at the edge does not scroll — a nudge continues it — and that is the trade every file manager
 * makes the other way and gets wrong. Writing `scrollTop` is enough here: the virtualizer reads
 * the same box, so the rows re-window as it moves.
 */
function edgeScroll(box: HTMLElement | null, y: number): void {
  if (box === null) return
  const rect = box.getBoundingClientRect()
  if (y < rect.top + EDGE) box.scrollTop -= EDGE_STEP
  else if (y > rect.bottom - EDGE) box.scrollTop += EDGE_STEP
}
