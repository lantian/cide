/**
 * The layout owns a slot; the window owns the drawing session. Project switches remove all
 * of a project's slots, while its open tabs remain in the workspace. Reconcile against that
 * workspace, including inactive projects, rather than treating an unmount as a close.
 */
import { useLayoutEffect, useRef, type ReactNode } from 'react'
import type { Bootstrap } from '@/ipc/client'
import { useWorkspace } from '@/store/workspace'
import { shownTabs } from '@/windows/windowTabs'
import { ExcalidrawPane, type ExcalidrawPaneProps } from './ExcalidrawPane'
import { drawingKindFor } from './excalidrawKinds'
import { createDrawingHosts } from './drawingHosts'

const hosts = createDrawingHosts((props: ExcalidrawPaneProps, attachment) => <ExcalidrawPane {...props} attachment={attachment} />)

/** All drawing panes owned by this window, not only the project it currently displays. */
export function drawingSessions(boot: Bootstrap): ReadonlyMap<string, ExcalidrawPaneProps> {
  const live = new Map<string, ExcalidrawPaneProps>()
  const { role, workspace } = boot
  if (role.kind === 'detachedPane') return live
  const projects = role.kind === 'shell' ? role.projects : [role.project]
  for (const id of projects) {
    const project = workspace.projects[id]
    if (project === undefined) continue
    for (const tab of shownTabs(role, project.tabs, workspace.windows)) {
      if (tab.kind.kind !== 'file' || drawingKindFor(tab.kind.path) === null) continue
      for (const pane of Object.values(tab.tree.panes)) {
        // A File tab can contain Claude splits; PaneBody only renders its editor pane.
        if (pane.kind !== 'editor') continue
        live.set(pane.id, {
          project: id,
          tab: tab.id,
          path: tab.kind.path,
          root: pane.harnessConversation?.cwd ?? pane.continues?.cwd ?? project.roots[0]?.path ?? '.',
          onScreen: role.kind === 'detachedTab' || (role.active === id && project.activeTab === tab.id),
        })
      }
    }
  }
  return live
}

let subscribed = false
let scheduled = false

function followWorkspace(): void {
  // One subscription per window, installed only after its first drawing mounts. A snapshot
  // may arrive while React commits a close; dispose separate roots after that stack unwinds.
  // Read the latest mirror once so a burst cannot replay a stale close or rename.
  const schedule = (): void => {
    if (scheduled) return
    scheduled = true
    queueMicrotask(() => {
      scheduled = false
      const boot = useWorkspace.getState().boot
      if (boot !== null) hosts.reconcile(drawingSessions(boot))
    })
  }
  if (!subscribed) {
    subscribed = true
    useWorkspace.subscribe((state, previous) => {
      if (state.boot !== previous.boot) schedule()
    })
  }
  schedule()
}

export function ExcalidrawHost({ pane, ...props }: ExcalidrawPaneProps & { pane: string }): ReactNode {
  const slot = useRef<HTMLDivElement>(null)
  const latest = useRef(props)
  latest.current = props
  const { project, tab } = props

  useLayoutEffect(() => {
    if (slot.current === null || project === undefined || tab === undefined) return
    const park = hosts.mount(pane, slot.current, latest.current)
    followWorkspace()
    return park
  }, [pane, project, tab])

  useLayoutEffect(() => {
    if (slot.current !== null) hosts.update(pane, slot.current, props)
  })

  // Standalone panes have no workspace identity to retain or reconcile.
  if (project === undefined || tab === undefined) return <ExcalidrawPane {...props} />
  return <div ref={slot} style={{ position: 'relative', width: '100%', height: '100%', minWidth: 0, minHeight: 0 }} />
}
