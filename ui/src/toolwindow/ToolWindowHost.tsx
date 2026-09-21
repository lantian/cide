/**
 * The git tool window's wiring: the workspace mirror in, commands out.
 *
 * Split from `ToolWindow.tsx` for the reason `GitPanelHost.tsx` is split from `GitPanel.tsx` —
 * the view stays server-renderable by a check script, and everything that needs a store, an
 * `invoke` or a live DOM is here.
 *
 * # Why there is no store
 *
 * The panel's state is **Rust-owned** — on `Project::tool_window`, or on
 * `Workspace::tool_window` for the shell with no project open, which is the same field in the
 * one place a project cannot be named — so the workspace mirror already *is* the store: a toggle bumps `rev`, `emit::workspace_changed` reaches every window, and
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
import { ToolWindowNoProject, ToolWindowView } from './ToolWindow'
import { ToolWindowSplitter } from './ToolWindowSplitter'
import {
  DOCKER_TAB,
  isDockerTab,
  isExtTab,
  restoreTabs,
  tabRow,
  walkTab,
  type HistoryTab,
} from './toolWindowModel'
// M22. Contributed tabs are not part of `ToolWindowState` and deliberately are not stored — see
// `ExtTab` in the model for why — so they arrive from the extension store and the active one is
// held here, in the webview, beside the other transient gesture state.
import { ExtPanelHost } from '@/ext/ExtPanelHost'
import { DockerPanel } from '@/sidebar/DockerPanel'
import { useExtPanels } from '@/ext/extStore'
import { useState } from 'react'

export interface ToolWindowHostProps {
  /**
   * Whose panel this is, or `null` for the shell that has **no project open**. (M74)
   *
   * That frame's state is `Workspace.toolWindow` rather than any project's, and every command
   * below takes the same `null` through to Rust — `cide_core::toolwindow` resolves it, so this
   * component holds no second answer about which panel a gesture is for.
   *
   * What a `null` cannot produce is a history tab: `openHistory` takes a repository and a path,
   * so there is no road from here to one. The Log tab therefore draws a sentence instead of a
   * list, and Docker draws what it always draws — the daemon is a property of the machine.
   */
  project: ProjectId | null
}

export function ToolWindowHost({ project }: ToolWindowHostProps) {
  const state = useWorkspace((s) =>
    project === null
      ? (s.boot?.workspace.toolWindow ?? null)
      : (s.boot?.workspace.projects[project]?.toolWindow ?? null),
  )

  const panels = useExtPanels().filter((panel) => panel.def.location === 'bottom')
  /*
   * Which contributed tab is in front, or `null` for "one of Rust's".
   *
   * Webview state, and it is one of the two things `store/workspace.ts`'s header says the webview
   * legitimately owns: transient gesture state. Rust cannot hold it, because `ToolWindowState`
   * has nowhere to put an id whose extension may not be installed on the next launch.
   *
   * Cleared whenever the extension that owned it goes away, so disabling an extension in another
   * window does not leave this one showing a tab that no longer exists.
   */
  const [extActive, setExtActive] = useState<string | null>(null)
  /*
   * Whether the Docker tab is in front. (M46)
   *
   * Webview state beside `extActive` and for a narrower reason — see `DOCKER_TAB`. Rust's own
   * `active` is left exactly where it was while this is true, so switching to Docker and back
   * returns to the history tab the user had open, which is the behaviour a contributed tab
   * already has.
   */
  const [dockerActive, setDockerActive] = useState(false)
  const shownExt =
    extActive !== null && panels.some((panel) => panel.view === extActive) ? extActive : null

  const onActivate = useCallback(
    (id: string | null) => {
      if (isDockerTab(id)) {
        // Never reaches Rust: `tool_window_activate` takes a uuid and `docker` is not one.
        setExtActive(null)
        setDockerActive(true)
        return
      }
      setDockerActive(false)
      if (isExtTab(id)) {
        // Never reaches Rust: `tool_window_activate` takes a `HistoryTabId`, which is a uuid, and
        // an `ext:` id is not one. Rust's own `active` is left exactly where it was, so switching
        // back to a contributed tab and away again returns to the history tab the user had open.
        setExtActive(id)
        return
      }
      setExtActive(null)
      void toolWindowApi.activate(project, id as HistoryTabId | null).catch(() => {})
    },
    [project],
  )
  const onClose = useCallback(
    (id: string) => {
      // Unreachable with no project — that panel has no history tab to close — and guarded
      // rather than asserted, because both commands below take a `ProjectId` and neither has
      // anything to answer without one.
      if (project === null) return
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
  const repaired = restoreTabs({
    open: state.open,
    history: state.history as readonly HistoryTab[],
    active: state.active,
  })
  const tabs = {
    ...repaired,
    ext: panels.map((panel) => ({ view: panel.view, label: panel.def.label })),
    // A contributed tab in front wins over Rust's `active`, which stays where it was — see
    // `onActivate`. `restoreTabs` has already repaired the stored half, and this cannot make it
    // invalid again because `shownExt` is only ever an id in `panels`.
    // Docker outranks a contributed tab, which outranks Rust's — the order the three are
    // *selected* in, so the last thing clicked is the thing in front.
    active: dockerActive ? DOCKER_TAB : (shownExt ?? repaired.active),
  }
  const rows = tabRow(tabs)
  const active = tabs.history.find((t) => t.id === tabs.active) ?? null
  const extPanel = panels.find((panel) => panel.view === shownExt) ?? null

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
        {dockerActive ? (
          /*
           * The machine's containers. (M46)
           *
           * `placement="bottom"` so the panel draws no title of its own — the tab strip above it
           * already says Docker, and two headings stacked is what a sidebar panel dropped into
           * the bottom panel looks like. It keeps its connection line and its Refresh, because
           * those are controls rather than a heading.
           *
           * Not keyed: unlike a Log or an extension tab there is only ever one of these, so a
           * remount would throw away a live board and re-read the daemon for nothing.
           */
          <DockerPanel placement="bottom" />
        ) : extPanel === null ? (
          project === null ? (
            // `LogTab` is not rendered with a `null` project rather than being taught to
            // tolerate one. It opens a repository list, a branch list, a walk and an
            // `fs-changed` subscription on mount, all keyed by project; every one of those
            // would need a "there is no project" arm, and each arm would be a second place
            // that can answer wrongly. The sentence is the whole of the projectless Log tab.
            <ToolWindowNoProject />
          ) : (
            <LogTab
              key={active?.id ?? 'log'}
              project={project}
              tab={walkTab(active?.id ?? null)}
              repo={active === null ? null : (active.repo as RepoId)}
              path={active === null ? null : active.path}
            />
          )
        ) : (
          // Keyed by the view id for the same reason `LogTab` is keyed by its tab: switching tabs
          // remounts rather than re-points, so one extension's rows are never on screen under
          // another's heading for a frame.
          <ExtPanelHost key={extPanel.view} binding={extPanel} placement="bottom" />
        )}
      </ToolWindowView>
    </>
  )
}
