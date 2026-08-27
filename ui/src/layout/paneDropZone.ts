/**
 * Which pane is lit up as a drop zone, and which band of it — shared across every pane frame
 * in the tab without a context and without a fresh object.
 *
 * A drag starts in one `PaneFrame` and has to be seen by all of them: the source dims, and
 * whichever pane the pointer is over draws a band on one edge. Three ways to do that, and two
 * of them are traps this codebase has already fallen into once:
 *
 * * **A React context** re-renders every consumer *and* everything between the provider and
 *   them, on every pointermove. The provider here would be the tab, so that is the whole pane
 *   tree — including six terminals — sixty times a second.
 * * **A store selector returning `{ target, edge }`** is the infinite-render loop
 *   `check:selectors` exists for: `useSyncExternalStore` compares snapshots with `Object.is`,
 *   so a freshly built object is never equal to the last one and the component re-renders for
 *   ever, ending at *Maximum update depth exceeded*, which unmounts the whole root.
 *
 * So the snapshot is **a string, per pane** — `paneDropMark(id)` — which is stable by
 * construction. `chrome/panelRequests.ts` returns a boolean for the same reason and says so.
 *
 * # What a re-render here costs, and why it is nothing
 *
 * `setPaneDrag` notifies; every mounted `PaneFrame` recomputes its own mark; React bails out on
 * every one whose string is unchanged. Moving from pane A's right band to pane B's left band
 * therefore re-renders exactly A and B.
 *
 * And a `PaneFrame` re-render **does not reach the terminal**. `children` is an element that
 * `App.tsx`'s `renderPane` already built; a state-driven re-render of the frame passes the very
 * same element object, so React bails on that subtree — no `PaneBody`, no `PaneSlot` effect, no
 * `ResizeObserver`, no `fit()` and no `session_resize`. That is the load-bearing claim of this
 * design, and it holds only for as long as the mark reaches the DOM as an attribute the
 * stylesheet reads. Rebuilding `children` from the mark would undo all of it.
 */
import { useSyncExternalStore } from 'react'
import type { DropEdge } from './paneMove'

/**
 * What one pane should draw right now.
 *
 * `'none'` when no drag is in flight or this pane is a bystander, `'source'` for the pane being
 * dragged, and one of the four edges for the pane the pointer is over. A primitive, deliberately
 * — see the header.
 */
export type PaneDropMark = 'none' | 'source' | DropEdge

interface DragState {
  readonly source: string
  readonly target: string | null
  readonly edge: DropEdge | null
}

let drag: DragState | null = null
const listeners = new Set<() => void>()

function emit(): void {
  for (const listener of listeners) listener()
}

/**
 * Publish the drag, or `null` to end it.
 *
 * Compares before it emits, so a pointermove that stays inside the same band notifies nobody
 * and re-renders nothing — which is what keeps a drag across a 2x3 of terminals free.
 */
export function setPaneDrag(next: DragState | null): void {
  const same =
    drag === null
      ? next === null
      : next !== null &&
        drag.source === next.source &&
        drag.target === next.target &&
        drag.edge === next.edge
  if (same) return
  drag = next
  emit()
}

/** Is a pane drag in flight? Used by the tab to suppress its own hover affordances. */
export function paneDragActive(): boolean {
  return drag !== null
}

/** The `subscribe` half. Module-level, so its identity is stable across renders. */
export function subscribePaneDrop(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/** The `getSnapshot` half: what this one pane should draw. Always a primitive. */
export function paneDropMark(pane: string): PaneDropMark {
  if (drag === null) return 'none'
  if (drag.source === pane) return 'source'
  if (drag.target === pane && drag.edge !== null) return drag.edge
  return 'none'
}

/**
 * The same answer for `useSyncExternalStore`'s third argument.
 *
 * React refuses to render a store-reading component on the server without one, and that is not
 * hypothetical here: several `check:*` scripts SSR the pane views under node. No drag can be in
 * flight at that point, so the module's own answer is the honest one.
 */
export function paneDropMarkServer(): PaneDropMark {
  return 'none'
}

/** What a `PaneFrame` calls. */
export function usePaneDropMark(pane: string): PaneDropMark {
  return useSyncExternalStore(
    subscribePaneDrop,
    () => paneDropMark(pane),
    paneDropMarkServer,
  )
}

/** Drop every listener and any drag in flight. For the check script only. */
export function __resetPaneDropZone(): void {
  drag = null
  listeners.clear()
}
