/**
 * The pointer half of dragging the Sessions tree's rows — a session into a group, a group into
 * another or beside one, a closed row onto the *Opened* tab. Every rule it applies is
 * `sessionsTree.ts`'s; this file is the mechanism, and it is the part no check can run. (M199)
 *
 * # A copy of the file tree's mechanism, not a generalisation of it
 *
 * `useTreeDrag.ts` is pointer events and not HTML5 drag-and-drop for reasons that hold here
 * verbatim — Tauri's native drop handler is on and swallows HTML5 drags on some engines, and one
 * app should have one drag mechanism — so this is the same machine: a 4px threshold, pointer
 * capture only once the press is a drag (a double-click opens a row), window listeners for the
 * life of the gesture, a capture-phase Escape, `elementFromPoint` for the hit test because a
 * captured pointer stops sending enter/leave to the rows beneath, the verdict in a ref rather
 * than in state (a state updater may be replayed, and a drop must fire once), and a spring timer
 * keyed on what the pointer rests on. It is a copy because that hook is welded to file paths,
 * desktop export and editor-tab drops, and none of that is a session.
 *
 * # The hit test
 *
 * A row carries `data-session-row` with its type and id (and, for a session, the group it is
 * drawn in). Over a **group** row, a carried *group* reads the top quarter as "place before" and
 * — on a folded group, which has nothing drawn under it — the bottom quarter as "place after";
 * the rest is "into". Over a **session** row, the target is the group it is drawn in, or the loose
 * area. Anywhere else in the tree is the loose area; over the *Opened* sub-tab, while *Closed*
 * shows, is the tab.
 */
import { useCallback, useEffect, useRef, useState, type RefObject } from 'react'

import type { GroupView, SessionView } from './sessionsModel'
import {
  dropOutcome,
  springKey,
  type DragLoad,
  type DropEdit,
  type DropOutcome,
  type DropTarget,
} from './sessionsTree'

/** Past this many pixels a press is a drag — the file tree's number. */
const THRESHOLD = 4
/** How near the tree's edge the pointer scrolls it, and by how much a move. */
const EDGE = 24
const EDGE_STEP = 6
/** How long the pointer rests on a folded group, or the *Opened* tab, before it opens. */
const SPRING_MS = 600

export interface SessionsDragState {
  load: DragLoad
  /** What the ghost names. */
  label: string
  x: number
  y: number
  target: DropTarget | null
  outcome: DropOutcome | null
}

export interface UseSessionsDragOptions {
  /** The tree's scrolling box: the loose area's extent and the edge scroll. */
  container: RefObject<HTMLElement | null>
  views: readonly SessionView[]
  groups: readonly GroupView[]
  /** Which list shows — the *Opened* tab is a target only from *Closed*. */
  sub: 'opened' | 'closed'
  labelOf: (load: DragLoad) => string
  /** Send the edit. Absent (a fixture, a server render) makes the tree inert, not pretend. */
  onDrop?: ((edit: DropEdit) => void) | undefined
  /** Open what the pointer rested on: `'tab'`, or `'group:<id>'`. */
  onSpring?: ((key: string) => void) | undefined
}

export interface SessionsDrag {
  state: SessionsDragState | null
  /** Put on every row. */
  onPointerDown: (e: React.PointerEvent, load: DragLoad) => void
  /** Whether the gesture ending now became a drag — a click that follows it is not a click. */
  dragged: () => boolean
}

function hitTest(
  under: Element | null,
  container: HTMLElement | null,
  load: DragLoad,
  sub: 'opened' | 'closed',
  y: number,
  groups: readonly GroupView[],
): DropTarget | null {
  if (under === null) return null
  if (under.closest('[data-sessions-tab="opened"]') !== null) return sub === 'closed' ? { kind: 'tab' } : null
  if (container === null || !container.contains(under) || sub !== 'opened') return null
  const row = under.closest<HTMLElement>('[data-session-row]')
  if (row === null) return { kind: 'root' }
  const id = row.dataset['id'] ?? ''
  if (row.dataset['sessionRow'] === 'group') {
    if (load.type === 'group') {
      const box = row.firstElementChild?.getBoundingClientRect() ?? row.getBoundingClientRect()
      const at = (y - box.top) / Math.max(1, box.height)
      if (at < 0.28) return { kind: 'line', id, where: 'before' }
      const folded = groups.find((g) => g.id === id)?.collapsed === true
      if (at > 0.72 && folded) return { kind: 'line', id, where: 'after' }
    }
    return { kind: 'group', id }
  }
  if (row.dataset['sessionRow'] === 'session') {
    const group = row.dataset['group'] ?? ''
    return group === '' ? { kind: 'root' } : { kind: 'group', id: group }
  }
  return { kind: 'root' }
}

export function useSessionsDrag(options: UseSessionsDragOptions): SessionsDrag {
  const [state, setState] = useState<SessionsDragState | null>(null)
  /** Read by the listeners, which live for one gesture and must see the current tree. */
  const live = useRef(options)
  live.current = options

  const gesture = useRef<{
    pointerId: number
    from: { x: number; y: number }
    load: DragLoad
    started: boolean
    outcome: DropOutcome | null
    spring: { key: string; timer: ReturnType<typeof setTimeout> } | null
    stop: () => void
  } | null>(null)
  const wasDrag = useRef(false)

  const finish = useCallback((commit: boolean) => {
    const active = gesture.current
    gesture.current = null
    if (active?.spring) clearTimeout(active.spring.timer)
    active?.stop()
    setState(null)
    const outcome = active?.outcome
    if (commit && outcome?.kind === 'move') live.current.onDrop?.(outcome.edit)
  }, [])

  const springOn = useCallback((key: string | null) => {
    const active = gesture.current
    if (active === null || active.spring?.key === key) return
    if (active.spring !== null) clearTimeout(active.spring.timer)
    active.spring = null
    if (key === null) return
    const timer = setTimeout(() => {
      const current = gesture.current
      if (current === null || current.spring?.key !== key) return
      current.spring = null
      live.current.onSpring?.(key)
    }, SPRING_MS)
    active.spring = { key, timer }
  }, [])

  const onPointerDown = useCallback(
    (e: React.PointerEvent, load: DragLoad) => {
      // Left button, unmodified, one gesture at a time, and only with somewhere to drop.
      if (e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) return
      if (gesture.current !== null || live.current.onDrop === undefined) return
      wasDrag.current = false
      const el = e.currentTarget
      if (!(el instanceof HTMLElement)) return

      const move = (ev: PointerEvent) => {
        const active = gesture.current
        if (active === null || ev.pointerId !== active.pointerId) return
        if (!active.started) {
          const far =
            Math.abs(ev.clientX - active.from.x) > THRESHOLD || Math.abs(ev.clientY - active.from.y) > THRESHOLD
          if (!far) return
          active.started = true
          wasDrag.current = true
          try {
            el.setPointerCapture(ev.pointerId)
          } catch {
            // Released already (a very fast click): the window listeners carry it regardless.
          }
        }
        const now = live.current
        const under = document.elementFromPoint(ev.clientX, ev.clientY)
        const target = hitTest(under, now.container.current, active.load, now.sub, ev.clientY, now.groups)
        const outcome = dropOutcome(active.load, target, now.views, now.groups)
        active.outcome = outcome
        setState({ load: active.load, label: now.labelOf(active.load), x: ev.clientX, y: ev.clientY, target, outcome })
        springOn(springKey(target, now.groups))
        edgeScroll(now.container.current, ev.clientY)
      }
      const up = (ev: PointerEvent) => {
        const active = gesture.current
        if (active === null || ev.pointerId !== active.pointerId) return
        finish(active.started)
      }
      const cancel = () => finish(false)
      const key = (ev: KeyboardEvent) => {
        if (ev.key !== 'Escape' || gesture.current?.started !== true) return
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
        load,
        started: false,
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
    [finish, springOn],
  )

  // Unmounted mid-drag — the rail switching panels — must not leave listeners and a timer behind.
  useEffect(() => () => finish(false), [finish])

  return { state, onPointerDown, dragged: () => wasDrag.current }
}

/** Scroll the tree when the pointer is at its edge — per move, not on a timer (`useTreeDrag`). */
function edgeScroll(box: HTMLElement | null, y: number): void {
  if (box === null) return
  const rect = box.getBoundingClientRect()
  if (y < rect.top + EDGE) box.scrollTop -= EDGE_STEP
  else if (y > rect.bottom - EDGE) box.scrollTop += EDGE_STEP
}
