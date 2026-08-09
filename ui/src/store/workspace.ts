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
import { requestCloseConfirm } from '@/chrome/closeConfirmStore'
import type { CloseScope } from '@/chrome/closeConfirm'
import {
  app as appApi,
  events,
  pane as paneApi,
  session as sessionApi,
  unsavedChanges,
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
  type SessionSummary,
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

/**
 * Run a close command, turning the domain's unsaved-work refusal into a confirmation.
 *
 * Returns true when the close was *not* performed and a dialog is now up; false when it
 * went through. Any other failure re-throws — a close that fails for an unrelated reason is
 * not something to swallow behind a dialog about unsaved files.
 *
 * A shared helper rather than a copy in each close path: the two are one sentence apart and
 * the difference between them is the scope word, so a copy would eventually differ in
 * whether it re-throws — and the version that swallows loses errors silently.
 */
async function refused(
  scope: CloseScope,
  run: () => Promise<unknown>,
  proceed: () => Promise<void>,
): Promise<boolean> {
  try {
    await run()
    return false
  } catch (e) {
    const unsaved = unsavedChanges(e)
    if (!unsaved) throw e
    requestCloseConfirm({ scope, unsaved, sessions: [], proceed })
    return true
  }
}

/**
 * The `Busy | AwaitingPermission` sessions bound to panes in one tab.
 *
 * Answers `[]` without an IPC call when the tab has no session bound at all, which is every
 * file tab and every diff tab. The session ids come from the local mirror and the *states*
 * from Rust, because only the hook server knows which are mid-turn — and `app.quitRequested`
 * already applies the `confirmCloseWithLiveSession` setting, so this inherits it.
 */
async function liveSessionsInTab(
  boot: Bootstrap | null,
  project: ProjectId,
  tab: TabId,
): Promise<SessionSummary[]> {
  const target = boot?.workspace.projects[project]?.tabs.find((t) => t.id === tab)
  if (!target) return []
  const bound = new Set(
    Object.values(target.tree.panes)
      .map((pane) => pane.session)
      .filter((session): session is SessionId => session !== null),
  )
  if (bound.size === 0) return []

  const decision = await appApi.quitRequested(project)
  return decision.blocking.filter((s) => bound.has(s.session))
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
  /**
   * Close a project. Confirms first when it holds unsaved edits or a session mid-turn.
   *
   * `force` is the answer coming back from that confirmation and is never passed by a call
   * site that has not shown it — see `closeTab`.
   */
  closeProject: (id: ProjectId, force?: boolean) => Promise<void>
  newClaudeTab: (project: ProjectId) => Promise<TabId>
  activateTab: (project: ProjectId, tab: TabId) => Promise<void>
  /**
   * Close a tab, confirming first if that would lose something.
   *
   * Call sites pass two arguments and get the guard for free: with unsaved edits in the tab
   * this parks a `PendingClose` and resolves without closing, and the dialog's answer calls
   * back in with `force: true`. The refusal itself comes from Rust, so a call site that
   * bypassed this store still could not discard a buffer.
   */
  closeTab: (project: ProjectId, tab: TabId, force?: boolean) => Promise<void>

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
  closeProject: async (id, force = false) => {
    // Asked *before* the command, unlike `closeTab`, because a project close can be blocked
    // by something Rust does not refuse: a Claude session mid-turn. Rust has no business
    // refusing that — an interrupted turn is recoverable and the user may have turned the
    // warning off — so the only way to warn about it is to ask.
    if (!force) {
      const decision = await appApi.quitRequested(id)
      if (decision.unsaved.length > 0 || decision.blocking.length > 0) {
        requestCloseConfirm({
          scope: 'project',
          unsaved: decision.unsaved,
          sessions: decision.blocking,
          proceed: () => get().closeProject(id, true),
        })
        return
      }
    }
    // Still guarded on the way out: a buffer can go dirty between the question and the
    // answer, and the refusal is the thing that makes that race harmless.
    const parked = await refused(
      'project',
      () => projectApi.close(id, force),
      () => get().closeProject(id, true),
    )
    if (parked) return
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
  closeTab: async (project, tab, force = false) => {
    // No pre-flight question here, unlike `closeProject`: the unsaved case comes back from
    // Rust as a refusal that already names the file, so asking first would be a second round
    // trip for an answer the failure path hands over anyway — and it would be the answer to
    // a slightly older workspace.
    //
    // Sessions are the exception, and they are asked about only when this tab actually has
    // one bound. A file tab has no session, and paying an IPC round trip to be told so on
    // every `×` is exactly the kind of cost that gets a guard removed later.
    if (!force) {
      const live = await liveSessionsInTab(get().boot, project, tab)
      if (live.length > 0) {
        requestCloseConfirm({
          scope: 'tab',
          unsaved: [],
          sessions: live,
          proceed: () => get().closeTab(project, tab, true),
        })
        return
      }
    }
    const parked = await refused(
      'tab',
      () => tabApi.close(project, tab, force),
      () => get().closeTab(project, tab, true),
    )
    if (parked) return
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
