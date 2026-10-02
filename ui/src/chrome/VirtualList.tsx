/** Measured rows with bounded DOM work, including wrapped labels and changing panel widths. */
import { useRef, type ReactNode } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'

export function VirtualList<T>({ items, itemKey, render, estimateSize, className }: {
  items: readonly T[]
  itemKey: (item: T) => string
  render: (item: T) => ReactNode
  estimateSize: (item: T) => number
  className?: string | undefined
}) {
  const scroll = useRef<HTMLDivElement>(null)
  const list = useVirtualizer({
    count: items.length,
    getScrollElement: () => scroll.current,
    getItemKey: (index) => itemKey(items[index]!),
    estimateSize: (index) => estimateSize(items[index]!),
    overscan: 12,
    initialRect: { width: 320, height: 500 },
  })
  return (
    <div ref={scroll} className={className} style={{ overflow: 'auto', minHeight: 0 }} data-audit="virtualList">
      <div style={{ height: list.getTotalSize(), width: '100%', position: 'relative' }}>
        {list.getVirtualItems().map((row) => (
          <div key={row.key} data-index={row.index} ref={list.measureElement}
            style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${row.start}px)` }}>
            {render(items[row.index]!)}
          </div>
        ))}
      </div>
    </div>
  )
}
