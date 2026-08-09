/**
 * The frontend's mirror of the Rust-owned workspace.
 *
 * A *mirror*, not a copy with opinions: nothing here mutates the tree. Every change goes
 * to Rust as a command and comes back as a new snapshot, because the domain has to survive
 * a window closing and a second window has to see the same state. Local writes would give
 * two windows two answers.
 *
 * The only genuinely local state is transient chrome: which overlay is open, whether a
 * splitter is mid-drag. Those never outlive the window and never need to agree with anyone.
 */
import { rememberSpawnPlan } from '@/layout/spawnPlans'
import { create } from 'zustand'
import { destroyHost, peekHost, releaseHost } from '@/layout/paneHosts'
import {
  app as appApi,
  events,
  pane as paneApi,
  session as sessionApi,
  windows as windowApi,
  project as projectApi,
  tab as tabApi,
  type Axis,
  type Bootstrap,
  type Direction,
  type PaneId,
  type Project,
  type ProjectId,
  type SessionId,
  type Side,
  type SplitId,
  type SplitIntent,
  type SplitOutcome,
  type Tab,
  type TabId,
  type WindowMode,
  type Workspace,
} from '@/ipc/client'

export type Theme = 'dark' | 'light'

/** Resolve once React has committed and the browser has laid out against it. */
function nextFrame(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
  })
}

interface WorkspaceStore {
  /** Null until the first `app.getBootstrap` resolves. */
  boot: Bootstrap | null
  theme: Theme

  hydrate: () => Promise<void>
  /** Start following `cide://workspace-changed`. Returns an unlisten function. */
  subscribe: () => Promise<() => void>
  /** Open a project and re-read the tree. */
  openProject: (paths: string[]) => Promise<void>
  closeProject: (id: ProjectId) => Promise<void>
  newClaudeTab: (project: ProjectId) => Promise<TabId>
  activateTab: (project: ProjectId, tab: TabId) => Promise<void>
  closeTab: (project: ProjectId, tab: TabId) => Promise<void>

  splitPane: (
    project: ProjectId,
    tab: TabId,
    pane: PaneId,
    axis: Axis,
    side: Side,
    intent?: SplitIntent | null,
  ) => Promise<SplitOutcome>
  closePane: (project: ProjectId, tab: TabId, pane: PaneId) => Promise<void>
  focusPane: (project: ProjectId, tab: TabId, pane: PaneId) => Promise<void>
  maximizePane: (project: ProjectId, tab: TabId, pane: PaneId | null) => Promise<void>
  /** Committed on pointerup only — the drag itself writes to the DOM. */
  setRatio: (project: ProjectId, tab: TabId, split: SplitId, ratio: number) => Promise<void>
  navigatePane: (
    project: ProjectId,
    tab: TabId,
    pane: PaneId,
    direction: Direction,
  ) => Promise<void>
  bindSession: (
    project: ProjectId,
    tab: TabId,
    pane: PaneId,
    session: SessionId,
  ) => Promise<void>

  /**
   * Tear a pane out into its own window.
   *
   * The host is *released*, not destroyed: the session stays alive in the Rust registry and
   * the new window attaches to it. Destroying would take the child with it, which is the
   * one thing detaching must never do.
   */
  detachPane: (project: ProjectId, tab: TabId, pane: PaneId) => Promise<void>
  redockPane: (label: string) => Promise<void>
  setWindowMode: (mode: WindowMode) => Promise<void>
  /** Replace the mirror. Older revisions are ignored — snapshots can race. */
  applySnapshot: (workspace: Workspace) => void
  setTheme: (theme: Theme) => void
  toggleTheme: () => void
}

export const useWorkspace = create<WorkspaceStore>((set, get) => ({
  boot: null,
  theme: 'dark',

  hydrate: async () => {
    const boot = await appApi.getBootstrap()
    set({ boot, theme: boot.workspace.settings.theme })
  },

  /**
   * Follow every mutation, including the ones this window did not make.
   *
   * Before M5 there was one window and a local re-read after each command was enough. A
   * detached pane makes that false: the pane has to vanish from the shell window and appear
   * in the new one, and neither window can learn that by asking about its own last command.
   */
  subscribe: async () => {
    const unlisten = await events.onWorkspaceChanged((workspace) => {
      get().applySnapshot(workspace)
    })
    return unlisten
  },

  // Every mutation re-reads the whole tree rather than patching the mirror locally. The
  // snapshot is small, the round trip is already paid for, and a local patch that drifts
  // from Rust's answer is a class of bug worth not having. `cide://workspace-changed`
  // replaces the re-read in a later milestone.
  openProject: async (paths) => {
    await projectApi.open(paths)
    await get().hydrate()
  },
  closeProject: async (id) => {
    await projectApi.close(id)
    await get().hydrate()
  },
  newClaudeTab: async (project) => {
    const created = await tabApi.newClaude(project)
    await get().hydrate()
    return created
  },
  activateTab: async (project, tab) => {
    await tabApi.activate(project, tab)
    await get().hydrate()
  },
  closeTab: async (project, tab) => {
    await tabApi.close(project, tab)
    await get().hydrate()
  },

  splitPane: async (project, tab, pane, axis, side, intent = null) => {
    const created = await paneApi.split(project, tab, pane, axis, side, intent)
    // Recorded before the hydrate that makes the pane renderable, so the spawn plan is
    // already in place by the time `TerminalPane` mounts and asks for one. The other order
    // races: the pane appears, spawns a fresh session, and the fork is lost.
    if (created.intent.kind === 'forkPrimary' || created.intent.kind === 'mirror') {
      rememberSpawnPlan(created.pane, created.intent)
    }
    await get().hydrate()
    return created
  },
  closePane: async (project, tab, pane) => {
    // Three steps, and the order is the whole of it.
    //
    // 1. The domain call, which may be refused — the project console's primary pane cannot
    //    go, nor a tab's last — so nothing may be disposed until it has succeeded.
    await paneApi.close(project, tab, pane)

    // 2. Re-read and let React commit. The pane's `TerminalPane` unmounts here: its effect
    //    cleanup detaches the sink, clears the exit poll and parks the host. Disposing
    //    before this point destroys a host that is still mounted, and any async
    //    continuation still in flight would call `getHost` and resurrect it — leaving a
    //    freshly created host carrying a session nobody is watching.
    await get().hydrate()
    await nextFrame()

    // 3. Only now is the pane genuinely finished, so the child and the host go with it.
    //    Leaving the host registered leaks an xterm instance and possibly a WebGL context
    //    per closed pane, and WebKitGTK caps concurrent contexts at roughly 8-16 — a
    //    session spent splitting and closing would stop painting. Detaching a pane into its
    //    own window is the opposite case and uses `releaseHost`, which keeps both.
    const session = peekHost(pane)?.sessionId
    destroyHost(pane)
    if (session) await sessionApi.kill(session)
  },
  focusPane: async (project, tab, pane) => {
    await paneApi.focus(project, tab, pane)
    await get().hydrate()
  },
  maximizePane: async (project, tab, pane) => {
    await paneApi.maximize(project, tab, pane)
    await get().hydrate()
  },
  setRatio: async (project, tab, split, ratio) => {
    await paneApi.setRatio(project, tab, split, ratio)
    await get().hydrate()
  },
  navigatePane: async (project, tab, pane, direction) => {
    // Two calls because the domain separates "where would focus go" from "move it": the
    // first is a pure query on the tree, and at the edge it answers null rather than
    // wrapping around, which is what stops Alt+Left cycling forever in a two-pane tab.
    const target = await paneApi.navigate(project, tab, pane, direction)
    if (target === null) return
    await paneApi.focus(project, tab, target)
    await get().hydrate()
  },
  bindSession: async (project, tab, pane, session) => {
    await paneApi.bindSession(project, tab, pane, session)
    await get().hydrate()
  },

  detachPane: async (project, tab, pane) => {
    // Measured before the pane leaves the tree, because a moment later its host is parked
    // and reports nothing. The new window then opens at the size the pane already was.
    const el = peekHost(pane)?.el
    const rect =
      el && el.clientWidth > 0 && el.clientHeight > 0
        ? { width: el.clientWidth, height: el.clientHeight }
        : undefined
    await windowApi.detachPane(project, tab, pane, rect)
    await get().hydrate()
    await nextFrame()
    // Released rather than destroyed. The pane has left this window's tree but its session
    // is still running and the new window is attaching to it; destroying the host here
    // would dispose a terminal whose child nobody has told to stop.
    releaseHost(pane)
  },
  redockPane: async (label) => {
    await windowApi.redockPane(label)
    await get().hydrate()
  },
  setWindowMode: async (mode) => {
    await windowApi.setMode(mode)
    await get().hydrate()
  },

  applySnapshot: (workspace) => {
    const current = get().boot
    if (!current) return
    // Events carry the revision precisely so a snapshot that arrives out of order can be
    // dropped rather than winding the UI backwards.
    if (workspace.rev < current.workspace.rev) return
    set({ boot: { ...current, workspace } })
  },

  setTheme: (theme) => set({ theme }),
  toggleTheme: () => set((s) => ({ theme: s.theme === 'dark' ? 'light' : 'dark' })),
}))

/** Projects in header-tab order. */
export function useProjects(): Project[] {
  return useWorkspace((s) => (s.boot ? Object.values(s.boot.workspace.projects) : []))
}

/** The project this window is currently showing, if any. */
export function useActiveProject(): Project | null {
  return useWorkspace((s) => {
    const boot = s.boot
    if (!boot || boot.role.kind !== 'shell' || !boot.role.active) return null
    return boot.workspace.projects[boot.role.active] ?? null
  })
}

/** The active project's tabs, `tabs[0]` being the pinned console. */
export function useTabs(): Tab[] {
  const project = useActiveProject()
  return project?.tabs ?? []
}
