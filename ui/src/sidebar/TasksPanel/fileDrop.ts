/**
 * Files dragged in from the desktop, for the task card and the New task dialog. (M39)
 *
 * # Why this is Tauri's event and not HTML5 drag-and-drop
 *
 * The native drag-drop handler on every cide window is **on** — `crates/cide-app/src/windows.rs`
 * never disables it — and it eats `dragenter`/`dragover`/`drop` before the page sees them, which
 * is why every in-app drag in this codebase is pointer-based and why `sidebar/useTreeDrag.ts`
 * records "no drag *into* cide from Dolphin or Nautilus" as an accepted cost. What the handler
 * takes from the page it hands back as events of its own, with the dropped **paths** — which is
 * exactly the currency an attachment needs, since Rust copies by path and no bytes may cross the
 * seam. So this is the one surface in cide that accepts a desktop drag, and it can because it
 * wants paths, not a `File`.
 *
 * # One subscription per window, one hit test
 *
 * `App.tsx` calls [`startFileDrop`] once. The event carries a **physical** position and nothing
 * about elements, so the target is found by geometry: every element carrying `data-attach-drop`
 * is measured and `dropZoneAt` picks the innermost one under the pointer (a comment inside the
 * card takes the file; the card takes it otherwise). The zones are measured on every event
 * rather than cached, because a drag lasts seconds and the card can scroll under it.
 *
 * # What crosses back to React
 *
 * A **string** — the hot zone's key, through [`useDropHot`] — on `layout/paneDropZone.ts`'s
 * rule: a primitive snapshot is `Object.is`-stable, so a `dragover` at 60 Hz that stays inside
 * one zone re-renders nothing. The drop itself is a callback registry, [`onFileDrop`]: the card's
 * host handles `task`/`comment`/`composer`, the panel host handles `compose`, and a drop on a
 * key nobody registered for is simply not a drop.
 */
import { useSyncExternalStore } from 'react'
import { dragDrop, diag } from '@/ipc/client'
import { cssPoint, dropZoneAt, parseDropTarget, type DropTarget, type DropZoneRect } from './model'

export type FileDropHandler = (target: DropTarget, paths: readonly string[]) => void

let hot: string | null = null
const listeners = new Set<() => void>()
const handlers = new Set<FileDropHandler>()

function emit(): void {
  for (const listener of listeners) listener()
}

function setHot(next: string | null): void {
  if (hot === next) return
  hot = next
  emit()
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

function currentHot(): string | null {
  return hot
}

function noHot(): null {
  return null
}

/** The `data-attach-drop` key under a desktop drag right now, or `null`. A primitive. */
export function useDropHot(): string | null {
  return useSyncExternalStore(subscribe, currentHot, noHot)
}

/** Be told about drops. Every registered handler hears every drop; each acts on its own kinds. */
export function onFileDrop(handler: FileDropHandler): () => void {
  handlers.add(handler)
  return () => {
    handlers.delete(handler)
  }
}

/** Every drop zone on screen, measured now, in CSS pixels. */
function zones(): DropZoneRect[] {
  const out: DropZoneRect[] = []
  for (const element of document.querySelectorAll('[data-attach-drop]')) {
    const key = element.getAttribute('data-attach-drop')
    if (key === null || key === '') continue
    const rect = element.getBoundingClientRect()
    out.push({ key, left: rect.left, top: rect.top, width: rect.width, height: rect.height })
  }
  return out
}

/**
 * Subscribe this window to Tauri's drag events. Once, from `App.tsx`; the answer unsubscribes.
 *
 * Never throws: a build whose webview does not report drags (or a permission that withholds
 * them) costs the drop gesture and nothing else — the picker and the clipboard still attach.
 * Said out loud because the failure is otherwise total silence, which is the class of failure
 * `docs/journal.md`'s M39 entry names as the one this feature cannot verify by reading.
 */
export async function startFileDrop(): Promise<() => void> {
  try {
    return await dragDrop.onEvent((event) => {
      switch (event.type) {
        case 'enter':
        case 'over': {
          const point = cssPoint(event.position, window.devicePixelRatio)
          setHot(dropZoneAt(point, zones()))
          break
        }
        case 'leave':
          setHot(null)
          break
        case 'drop': {
          const point = cssPoint(event.position, window.devicePixelRatio)
          const key = dropZoneAt(point, zones())
          setHot(null)
          if (key === null) return
          const target = parseDropTarget(key)
          if (target === null || event.paths.length === 0) return
          for (const handler of handlers) handler(target, event.paths)
          break
        }
      }
    })
  } catch (error: unknown) {
    diag.log(`file drop: not available in this window: ${String(error)}`)
    return () => {}
  }
}
