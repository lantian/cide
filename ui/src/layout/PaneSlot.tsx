/**
 * Positions a pane host without ever owning its DOM.
 *
 * The slot is an empty positioned box; `mountHost` moves the pane's long-lived element
 * into it on mount and parks it again on unmount. React never renders the pane's contents,
 * so a re-render, a re-parent or a move between splits cannot destroy a terminal.
 */
import { useLayoutEffect, useRef } from 'react'
import { mountHost, parkHost } from './paneHosts'

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

  useLayoutEffect(() => {
    const slot = ref.current
    if (!slot) return

    mountHost(paneId, slot)

    const ro = new ResizeObserver((entries) => {
      const box = entries[0]?.contentRect
      if (box && box.width > 0 && box.height > 0) {
        onResizeRef.current?.(box.width, box.height)
      }
    })
    ro.observe(slot)

    return () => {
      ro.disconnect()
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
