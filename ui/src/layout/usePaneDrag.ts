/**
 * The pointer half of the pane grab handle: press, aim, drop.
 *
 * Everything decidable lives in `paneMove.ts`, which a check script can run under node; what is
 * here is listeners, refs and capture — the part no test in this repo can execute, so it is kept
 * as small as it can be made.
 *
 * # Pointer events, never HTML5 drag-and-drop
 *
 * The same rule the tab strip, the file tree and the git panel already follow, and it is not
 * stylistic: Tauri's native drag-drop handler is on — `windows.rs` never calls
 * `disable_drag_drop_handler()` — so `dragstart` and friends are intercepted by the webview
 * before the page sees them. A `draggable` attribute here would produce a gesture that works in
 * a browser and does nothing in the app.
 *
 * # Why the hit test is scoped to one tree, and what happens without it
 *
 * `TabContent` lays **every** tab out at full size and hides the inactive ones with
 * `visibility: hidden` — never `display: none`, which would zero the measurements xterm reads.
 * So `document.querySelectorAll('[data-pane-id]')` answers with the panes of every open tab, at
 * real, overlapping coordinates, and a drop would land in whichever one the query happened to
 * reach first — silently, in a tab the user cannot see. The scan is therefore rooted at the
 * `[data-audit="paneTree"]` element the gesture started inside, which is exactly the active
 * tab's tree. A press with no such ancestor — the detached-pane window, which renders a frame
 * with no `SplitTree` above it — starts no gesture at all.
 *
 * # The rects are read once
 *
 * At drag start, not per move. Nothing in the layout changes until the drop, and re-reading
 * `getBoundingClientRect` on every pointermove is a forced synchronous reflow over a grid of
 * live terminals.
 */
import { useCallback, useEffect, useRef } from 'react'
import { lockBodyForDrag, unlockBodyAfterDrag } from '@/chrome/dragLock'
import { beginResizeGesture, endResizeGesture } from './resizeGesture'
import { setPaneDrag } from './paneDropZone'
import {
  THRESHOLD,
  dropOutcome,
  dropTarget,
  type MoveNode,
  type MoveOutcome,
  type PaneBox,
} from './paneMove'

export interface UsePaneDragOptions {
  /** The pane this handle belongs to. */
  pane: string
  /** The tab's tree, read at drop time so a snapshot landing mid-drag is respected. */
  tree: () => MoveNode | null
  /**
   * Commit the move. Absent makes the handle inert rather than pretending — a drag that cannot
   * land is a drag that should not start, the rule `useTabDrag` states for its own commit.
   */
  onMove?: ((outcome: MoveOutcome) => void) | undefined
}

interface Gesture {
  readonly pointer: number
  readonly origin: { x: number; y: number }
  readonly boxes: readonly PaneBox[]
  readonly target: EventTarget & Element
  started: boolean
  outcome: MoveOutcome | null
}

/** Read every pane's box out of one tab's tree, in DOM order. */
function boxesIn(root: Element): PaneBox[] {
  const out: PaneBox[] = []
  for (const el of root.querySelectorAll('[data-pane-id]')) {
    const id = el.getAttribute('data-pane-id')
    if (id === null) continue
    const r = el.getBoundingClientRect()
    out.push({ pane: id, left: r.left, top: r.top, width: r.width, height: r.height })
  }
  return out
}

export function usePaneDrag({ pane, tree, onMove }: UsePaneDragOptions): {
  onPointerDown: (event: React.PointerEvent) => void
} {
  const gesture = useRef<Gesture | null>(null)

  const finish = useCallback((commit: boolean) => {
    const live = gesture.current
    gesture.current = null
    if (live === null) return

    if (live.started) {
      setPaneDrag(null)
      unlockBodyAfterDrag()
      // Paired with the `begin` in `start` below: the whole gesture is one resize as far as
      // the terminals are concerned, so they refit once at the end rather than per frame.
      endResizeGesture()
    }
    try {
      live.target.releasePointerCapture(live.pointer)
    } catch {
      // The pointer is already gone — a cancel, or the element was unmounted under it.
    }
    // Read from the ref rather than from React state, and written synchronously by the last
    // move: the StrictMode replay trap `useTabDrag` documents, where a state update queued by
    // the final pointermove has not landed by the time pointerup runs.
    if (commit && live.outcome !== null) onMove?.(live.outcome)
  }, [onMove])

  // A gesture can be taken off screen mid-drag — by a snapshot that closes this pane, or by a
  // project switch. Without this the body keeps a `grabbing` cursor and an unselectable page
  // for the rest of the session, the failure `dragLock`'s own header describes.
  useEffect(() => () => finish(false), [finish])

  const onPointerDown = useCallback(
    (event: React.PointerEvent) => {
      if (event.button !== 0 || onMove === undefined) return
      const handle = event.currentTarget
      const root = handle.closest('[data-audit="paneTree"]')
      // No tree above this frame — a detached-pane window. Nothing to move it within.
      if (root === null) return

      // The cluster's own `onMouseDown` already prevents the default to keep DOM focus in the
      // terminal; this stops the press from also reaching the frame's focus-raise.
      event.stopPropagation()

      const live: Gesture = {
        pointer: event.pointerId,
        origin: { x: event.clientX, y: event.clientY },
        boxes: boxesIn(root),
        target: handle,
        started: false,
        outcome: null,
      }
      gesture.current = live

      const start = (): void => {
        live.started = true
        try {
          handle.setPointerCapture(live.pointer)
        } catch {
          // Capture is an optimisation here — the window listeners below carry the gesture.
        }
        lockBodyForDrag('grabbing')
        beginResizeGesture()
        setPaneDrag({ source: pane, target: null, edge: null })
      }

      const move = (e: PointerEvent): void => {
        if (e.pointerId !== live.pointer) return
        if (!live.started) {
          const far =
            Math.abs(e.clientX - live.origin.x) >= THRESHOLD ||
            Math.abs(e.clientY - live.origin.y) >= THRESHOLD
          if (!far) return
          start()
        }
        const hit = dropTarget(live.boxes, e.clientX, e.clientY, pane)
        const root = tree()
        // A drop that would change nothing paints nothing, so the user is never invited to
        // perform the no-op re-flow `paneMove`'s header describes.
        const outcome =
          hit === null || root === null ? null : dropOutcome(root, pane, hit.target, hit.edge)
        live.outcome = outcome
        setPaneDrag({
          source: pane,
          target: outcome === null ? null : outcome.target,
          edge: outcome === null ? null : hit?.edge ?? null,
        })
      }

      /*
       * Every exit runs through here, and it detaches before it commits.
       *
       * Escape is the reason this is one function rather than a teardown hung off `pointerup`:
       * a cancelled drag never sees a pointerup at all, so listeners removed only there would
       * outlive the gesture and the next press would drive two state machines at once.
       */
      const stop = (commit: boolean): void => {
        window.removeEventListener('pointermove', move)
        window.removeEventListener('pointerup', up)
        window.removeEventListener('pointercancel', cancel)
        window.removeEventListener('keydown', key, true)
        finish(commit)
      }

      function up(e: PointerEvent): void {
        if (e.pointerId !== live.pointer) return
        stop(true)
      }
      function cancel(e: PointerEvent): void {
        if (e.pointerId !== live.pointer) return
        stop(false)
      }
      // Capture phase: a terminal or an overlay must not eat the Escape that abandons a drag.
      function key(e: KeyboardEvent): void {
        if (e.key !== 'Escape') return
        e.preventDefault()
        e.stopPropagation()
        stop(false)
      }

      window.addEventListener('pointermove', move)
      window.addEventListener('pointerup', up)
      window.addEventListener('pointercancel', cancel)
      window.addEventListener('keydown', key, true)
    },
    [pane, tree, onMove, finish],
  )

  return { onPointerDown }
}
