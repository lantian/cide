/**
 * The bridge between the pane audit and the running application.
 *
 * `paneAudit.ts` deliberately knows nothing about the store or the IPC client so it can be
 * tested against a fake; this is the adapter that points it at the real thing. It lives
 * outside the audit for that reason, and outside `App.tsx` because a 100-cycle test harness
 * has no business in the render path.
 */
import type { PaneAuditDriver, PaneAuditTree } from './paneAudit'
import { useWorkspace } from '@/store/workspace'
import type { Axis, PaneId, ProjectId, Tab, TabId } from '@/ipc/client'

/** Resolve once React has committed and the browser has laid out against it. */
function settle(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
  })
}

function activeProject() {
  const boot = useWorkspace.getState().boot
  if (!boot || boot.role.kind !== 'shell' || !boot.role.active) return null
  return boot.workspace.projects[boot.role.active] ?? null
}

function activeTabOf(): Tab | null {
  const project = activeProject()
  if (!project) return null
  return project.tabs.find((t) => t.id === project.activeTab) ?? null
}

/**
 * Build a driver over the live app.
 *
 * Returns `null` when no project is open — the audit needs somewhere to split, and a run
 * against an empty workspace would pass by doing nothing at all.
 */
export function createAppPaneDriver(): PaneAuditDriver | null {
  if (!activeProject()) return null

  const ids = (): { project: ProjectId; tab: TabId } => {
    const project = activeProject()
    const tab = activeTabOf()
    if (!project || !tab) throw new Error('pane audit: the active project or tab went away')
    return { project: project.id, tab: tab.id }
  }

  return {
    tree(): PaneAuditTree {
      const tab = activeTabOf()
      if (!tab) throw new Error('pane audit: no active tab')
      return {
        panes: Object.keys(tab.tree.panes),
        focused: tab.tree.focused,
        maximized: tab.tree.maximized,
      }
    },

    tabs: () => activeProject()?.tabs.map((t) => t.id) ?? [],
    activeTab: () => activeProject()?.activeTab ?? '',

    async activateTab(tab: string) {
      const { project } = ids()
      await useWorkspace.getState().activateTab(project, tab as TabId)
    },

    async split(pane: string, axis: 'row' | 'col') {
      const { project, tab } = ids()
      // `null` intent asks the domain for the tab's own default, which is the behaviour a
      // user gets; forcing one here would test a path the app never takes.
      return useWorkspace
        .getState()
        .splitPane(project, tab, pane as PaneId, axis as Axis, 'after', null)
    },

    async close(pane: string) {
      const { project, tab } = ids()
      await useWorkspace.getState().closePane(project, tab, pane as PaneId)
    },

    async focus(pane: string) {
      const { project, tab } = ids()
      await useWorkspace.getState().focusPane(project, tab, pane as PaneId)
    },

    async maximize(pane: string | null) {
      const { project, tab } = ids()
      await useWorkspace.getState().maximizePane(project, tab, pane as PaneId | null)
    },

    settle,
  }
}
