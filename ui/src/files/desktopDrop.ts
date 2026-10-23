/** One native event stream, routed to the visible element under the pointer. */
import { useEffect, useRef } from 'react'
import { dragDrop, type DragDropEvent, type ProjectId } from '@/ipc/client'
import { notifyFailure } from '@/chrome/notices'
import type { TreeDragSet } from '@/sidebar/treeDrag'

export interface DesktopHover {
  paths: readonly string[]
  x: number
  y: number
  /** Present only for a tree drag handed to the desktop by this webview. */
  source: TreeDragSet | null
}

interface Receiver {
  hover: (event: DesktopHover | null) => void
  drop: (event: DesktopHover) => void
}

const receivers = new Map<HTMLElement, Receiver>()
let entered: readonly string[] = []
let source: TreeDragSet | null = null
let hot: Receiver | null = null

export function registerDesktopDrop(element: HTMLElement, receiver: Receiver): () => void {
  receivers.set(element, receiver)
  return () => {
    receivers.delete(element)
    if (hot === receiver) { hot.hover(null); hot = null }
  }
}

export function useDesktopDrop(
  container: React.RefObject<HTMLElement | null>, receiver: Receiver,
): void {
  const live = useRef(receiver)
  live.current = receiver
  useEffect(() => {
    if (container.current === null) return
    return registerDesktopDrop(container.current, {
      hover: (event) => live.current.hover(event),
      drop: (event) => live.current.drop(event),
    })
  }, [container])
}

/** Called from the window's existing native subscription, alongside task attachments. */
export function routeDesktopDrop(event: DragDropEvent): void {
  if (event.type === 'leave') {
    hot?.hover(null); hot = null; entered = []
    return
  }
  if (event.type === 'enter' || event.type === 'drop') entered = event.paths
  if (entered.length === 0 && source !== null) entered = source.paths
  const x = event.position.x / dragDrop.coordinateScale()
  const y = event.position.y / dragDrop.coordinateScale()
  let element = document.elementFromPoint(x, y)
  let receiver: Receiver | null = null
  while (element !== null) {
    receiver = receivers.get(element as HTMLElement) ?? null
    if (receiver !== null) break
    element = element.parentElement
  }
  if (hot !== receiver) hot?.hover(null)
  hot = receiver
  const hover = { paths: entered, x, y, source }
  if (event.type === 'drop') {
    hot?.hover(null); hot = null; entered = []
    if (hover.paths.length > 0) receiver?.drop(hover)
  } else {
    receiver?.hover(hover)
  }
}

export async function exportTreeDrag(project: ProjectId, drag: TreeDragSet): Promise<void> {
  source = drag
  try { await dragDrop.start(project, drag.paths) }
  catch (error) { notifyFailure(error, { project }) }
  finally {
    source = null; entered = []; hot?.hover(null); hot = null
  }
}
