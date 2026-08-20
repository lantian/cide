/**
 * The git tool window's wiring: the workspace mirror in, commands out.
 *
 * Split from `ToolWindow.tsx` for the reason `GitPanelHost.tsx` is split from `GitPanel.tsx` —
 * the view stays server-renderable by a check script, and everything that needs a store, an
 * `invoke` or a live DOM is here.
 *
 * # Why there is no store
 *
 * The panel's state is **Rust-owned**, on `Project::tool_window`, so the workspace mirror already
 * *is* the store: a toggle bumps `rev`, `emit::workspace_changed` reaches every window, and
 * `store/workspace.ts` mirrors it. A zustand store beside that would be a second copy of one
 * fact, and `chrome/sidebarView.ts` records what happens when a boolean has two sources. The
 * commands are therefore called straight from here and from `keys/dispatch.ts`, and neither holds
 * any state of its own.
 *
 * The one thing the webview does own is the *painted* height, which has to be on the first frame
 * — see `ToolWindowSplitter.tsx`'s boot cache for why an IPC round trip is too late.
 */
import { useCallback } from 'react'
import { logWalk, toolWindow as toolWindowApi } from '@/ipc/client'
import type { HistoryTabId, ProjectId, RepoId } from '@/ipc/client'
import { useWorkspace } from '@/store/workspace'
import { LogTab } from '@/gitlog/LogTab'
import { ToolWindowView } from './ToolWindow'
import { ToolWindowSplitter } from './ToolWindowSplitter'
import { restoreTabs, tabRow, walkTab, type HistoryTab } from './toolWindowModel'

export interface ToolWindowHostProps {
  project: ProjectId
}

export function ToolWindowHost({ project }: ToolWindowHostProps) {
  const state = useWorkspace((s) => s.boot?.workspace.projects[project]?.toolWindow ?? null)

  const onActivate = useCallback(
    (id: string | null) => {
      void toolWindowApi.activate(project, id as HistoryTabId | null).catch(() => {})
    },
    [project],
  )
  const onClose = useCallback(
    (id: string) => {
      // Cancel first, then close. A walk over a deep repository outlives the tab that asked for
      // it by seconds, and nothing else would ever stop it: the registry's other cancellation is
      // *supersession* — the next `page` for the same key — and a closed tab issues no next page.
      // Without this the job runs to exhaustion holding a `LogQuery` nobody will read the answer
      // to, and `finish` only reclaims the slot once it gets there.
      //
      // Not awaited, and the close does not wait for it: cancellation is a stored atomic that the
      // walk observes on its next commit, so there is nothing to sequence, and making the tab's
      // disappearance wait on a round trip would make closing feel slower the deeper the repo.
      void logWalk.cancel(project, id).catch(() => {})
      void toolWindowApi.closeHistory(project, id as HistoryTabId).catch(() => {})
    },
    [project],
  )
  const onHide = useCallback(() => {
    void toolWindowApi.setLayout(project, { open: false }).catch(() => {})
  }, [project])

  if (state === null) return null

  // Repaired before it is drawn: a hand-edited `workspace.json` can hold an `active` naming no
  // tab, which would light nothing and show an empty body with no way back. `validate` refuses to
  // *write* that state; this is what stops one already on disk from reaching the screen.
  const tabs = restoreTabs({
    open: state.open,
    history: state.history as readonly HistoryTab[],
    active: state.active,
  })
  const rows = tabRow(tabs)
  const active = tabs.history.find((t) => t.id === tabs.active) ?? null

  return (
    <>
      <ToolWindowSplitter project={project} />
      <ToolWindowView rows={rows} onActivate={onActivate} onClose={onClose} onHide={onHide}>
        {/*
         * One component for both kinds of tab. The Log tab walks every repository in the project
         * (`repo: null`), a History tab walks the one its file lives in and filters by that path
         * — which is the same single field the backend query differs by, and keeping them one
         * component is what stops the two drifting into two answers for "which commits touched
         * this".
         *
         * Keyed by the tab, so switching tabs remounts rather than re-pointing: the fetch, the
         * generation counter and the selection are all per question, and re-pointing would leave
         * one tab's rows on screen under another tab's heading for a frame.
         */}
        <LogTab
          key={active?.id ?? 'log'}
          project={project}
          tab={walkTab(active?.id ?? null)}
          repo={active === null ? null : (active.repo as RepoId)}
          path={active === null ? null : active.path}
        />
      </ToolWindowView>
    </>
  )
}
