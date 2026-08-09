/**
 * Points the window audit at the running application.
 *
 * `windowAudit.ts` knows nothing about the store or the IPC client so it can be driven from
 * a fixture; this is the adapter to the real thing, and it lives outside both for the same
 * reason `appPaneDriver.ts` does.
 */
import type { WindowAuditDriver, WindowAuditMode, WindowAuditSnapshot } from './windowAudit'
import { useWorkspace } from '@/store/workspace'
import { session as sessionApi, windowLabel } from '@/ipc/client'
import type { PaneId, ProjectId, TabId, WindowMode } from '@/ipc/client'

function settle(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
  })
}

/**
 * Flatten the workspace into what the audit asserts over.
 *
 * `panes` deliberately merges both halves of the domain's split — every tab's tree and the
 * project's `detached` holding map — with `tab: null` marking the detached ones. The whole
 * question the audit asks of a detached pane is which of the two it is currently in, and
 * that is only answerable if both are in one list.
 */
function snapshot(): WindowAuditSnapshot {
  const boot = useWorkspace.getState().boot
  const projects = boot ? Object.values(boot.workspace.projects) : []

  return {
    mode: (boot?.workspace.settings.windowMode ?? 'stacked') as WindowAuditMode,
    projects: projects.map((p) => ({
      id: p.id,
      name: p.name,
      tabs: p.tabs.map((t) => t.id),
      panes: [
        ...p.tabs.flatMap((t) =>
          Object.values(t.tree.panes).map((pane) => ({
            id: pane.id,
            tab: t.id as string | null,
            session: pane.session,
          })),
        ),
        ...Object.values(p.detached).map((pane) => ({
          id: pane.id,
          tab: null,
          session: pane.session,
        })),
      ],
    })),
    windows: Object.entries(boot?.workspace.windows ?? {}).map(([label, role]) => ({
      label,
      kind: role.kind,
      projects: role.kind === 'shell' ? role.projects : [role.project],
    })),
  }
}

/**
 * Build a driver over the live app.
 *
 * Returns `null` when no project is open: an audit with nothing to detach would pass by
 * doing nothing, which is the one answer it must never give.
 */
export function createAppWindowDriver(): WindowAuditDriver | null {
  if (snapshot().projects.length === 0) return null

  return {
    snapshot,
    windowLabel,

    // The registry, not the tree. A pane that lost its session still names it.
    sessions: () => sessionApi.list(),
    exited: (session) => sessionApi.hasExited(session),

    async detach(project, tab, pane) {
      const before = new Set(Object.keys(useWorkspace.getState().boot?.workspace.windows ?? {}))
      await useWorkspace.getState().detachPane(project as ProjectId, tab as TabId, pane as PaneId)
      // The command returns the label, but the store swallows it to keep every mutation's
      // signature uniform; the new window is whichever one the workspace gained.
      const after = Object.keys(useWorkspace.getState().boot?.workspace.windows ?? {})
      const fresh = after.find((label) => !before.has(label))
      if (!fresh) throw new Error('detach produced no new window')
      return fresh
    },

    async redock(window) {
      await useWorkspace.getState().redockPane(window)
    },

    async setMode(mode) {
      await useWorkspace.getState().setWindowMode(mode as WindowMode)
    },

    async mirrorBytes(session) {
      const state = await sessionApi.scrollback(session)
      return state.byteLength
    },

    inAlternateScreen: (session) => sessionApi.inAlternateScreen(session),

    settle,
  }
}
