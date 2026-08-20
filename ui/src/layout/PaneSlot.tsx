/**
 * Positions a pane host without ever owning its DOM.
 *
 * The slot is an empty positioned box; `mountHost` moves the pane's long-lived element
 * into it on mount and parks it again on unmount. React never renders the pane's contents,
 * so a re-render, a re-parent or a move between splits cannot destroy a terminal.
 */
import { useLayoutEffect, useRef } from 'react'
import { mountHost, parkHost } from './paneHosts'
import { cancelResizeSettle, whenResizeSettles } from './resizeGesture'

export interface PaneSlotProps {
  paneId: string
  /** Called after mount and on every resize, with the slot's live pixel size. */
  onResize?: ((width: number, height: number) => void) | undefined
  className?: string | undefined
}

export function PaneSlot({ paneId, onResize, className }: PaneSlotProps) {
  const ref = useRef<HTMLDivElement>(null)
  // Kept in a ref so changing the callback does not tear down and re-observe.
  const onResizeRef = useRef(onResize)
  onResizeRef.current = onResize
  /**
   * The last size the observer reported, so a deferred callback reports the size the slot
   * ended at rather than the one it was passing through when the gesture started.
   */
  const lastBox = useRef({ width: 0, height: 0 })

  useLayoutEffect(() => {
    const slot = ref.current
    if (!slot) return

    mountHost(paneId, slot)

    const ro = new ResizeObserver((entries) => {
      const box = entries[0]?.contentRect
      if (!box || box.width <= 0 || box.height <= 0) return
      lastBox.current = { width: box.width, height: box.height }
      /*
       * Through `resizeGesture`, and this one line is the fix for the whole class.
       *
       * The only consumer of `onResize` is `TerminalPane`'s `syncSize`, which calls
       * `FitAddon.fit()` unconditionally — a DOM-renderer reflow over 5000 lines of
       * scrollback — and then a synchronous `session_resize` that reflows the vt100 mirror on
       * the IPC thread. A `ResizeObserver` fires once per frame of a drag, `TabContent` lays
       * every tab out at full size so *every* tab's panes are observed at once, and WebKitGTK
       * reports pointer moves at the mouse's rate rather than the frame rate. Deferring the
       * whole thing to the end of the gesture is what turns dozens of refits per pane per drag
       * into one.
       *
       * Keyed by pane id, so a pane that reports two hundred times refits once.
       */
      whenResizeSettles(paneId, () =>
        onResizeRef.current?.(lastBox.current.width, lastBox.current.height),
      )
    })
    ro.observe(slot)

    return () => {
      ro.disconnect()
      // Before `parkHost`, and not optional: `syncSize` reaches `getHost`, which builds a host
      // for a pane that has none — so a flush landing after this slot has gone would resurrect
      // one for a pane no longer in the tree.
      cancelResizeSettle(paneId)
      // Park, never remove. This is the difference between switching tabs and losing a
      // running Claude turn.
      parkHost(paneId)
    }
  }, [paneId])

  // `width/height: 100%` is load-bearing, not decoration. The host element inside is
  // `position: absolute; inset: 0`, so it is out of flow and contributes no height; a slot
  // sized by its content would therefore collapse to zero and the host would resolve
  // `inset: 0` against nothing. The symptom is subtle — widths look right, heights are 0,
  // and every terminal silently sits at its fallback geometry.
  //
  // The parent must have a definite size. In practice it always does: a pane body is a
  // flex item with `flex: 1`.
  return (
    <div
      ref={ref}
      className={className}
      style={{ position: 'relative', width: '100%', height: '100%', minWidth: 0, minHeight: 0 }}
    />
  )
}
