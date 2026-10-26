import { useEffect, useRef, useState } from 'react'
import { dragDrop } from '@/ipc/client'
import { useDesktopDrop, type DesktopHover } from '@/files/desktopDrop'
import { dropOutcome, externalDropOutcome, plural, springTarget, type DragRow } from './treeDrag'
import type { TreeDragState } from './useTreeDrag'

interface Options {
  container: React.RefObject<HTMLDivElement | null>
  roots: readonly string[]
  writable: readonly string[]
  rowFor: (path: string) => DragRow | null
  onSpring: (path: string) => void
  onMove: (paths: readonly string[], dest: string) => void
  onCopy: (paths: readonly string[], dest: string, receipt: string) => void
  onError: (error: unknown) => void
}

export function useTreeDesktopDrop(options: Options): TreeDragState | null {
  const [state, setState] = useState<TreeDragState | null>(null)
  const [spring, setSpring] = useState<string | null>(null)
  const live = useRef(options)
  live.current = options

  const aim = (event: DesktopHover) => {
    const under = document.elementFromPoint(event.x, event.y)
    const path = under?.closest<HTMLElement>('[data-row-path]')?.dataset['rowPath']
    // Empty space belongs to the root, only after the router has hit-tested this tree.
    const root = options.roots[0]
    const target = path !== undefined ? options.rowFor(path)
      : root === undefined ? null : { path: root, name: root.split('/').pop() ?? root,
          kind: 'dir' as const, expanded: true, hasChildren: true }
    const outcome = event.source === null
      ? externalDropOutcome(event.paths, target, options.roots, options.writable)
      : dropOutcome(event.source, target, options.roots, options.writable)
    return { target, outcome }
  }

  useDesktopDrop(options.container, {
    hover(event) {
      if (event === null) { setState(null); setSpring(null); return }
      const { target, outcome } = aim(event)
      const drag = event.source ?? { paths: event.paths, origin: '',
        label: event.paths.length === 1 ? event.paths[0]?.split('/').pop() ?? '' : plural(event.paths.length),
        widened: event.paths.length > 1, refusal: null }
      setState({ drag, x: event.x, y: event.y, outcome })
      setSpring(springTarget(target, outcome))
      const box = options.container.current
      if (box !== null) {
        const rect = box.getBoundingClientRect()
        if (event.y < rect.top + 24) box.scrollTop -= 6
        else if (event.y > rect.bottom - 24) box.scrollTop += 6
      }
    },
    drop(event) {
      const { outcome } = aim(event)
      if (outcome.kind !== 'move') return
      if (event.source !== null) { options.onMove(outcome.paths, outcome.destDir); return }
      void dragDrop.claim(event.paths)
        .then((receipt) => options.onCopy(event.paths, outcome.destDir, receipt))
        .catch(options.onError)
    },
  })

  useEffect(() => {
    if (spring === null) return
    const timer = setTimeout(() => {
      const row = live.current.rowFor(spring)
      if (row?.kind === 'dir' && !row.expanded) live.current.onSpring(spring)
    }, 600)
    return () => clearTimeout(timer)
  }, [spring])
  return state
}
