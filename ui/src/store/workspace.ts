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
import { planFileIndex, type IndexTarget } from './fileIndex'
import {
  app as appApi,
  events,
  fs as fsApi,
  pane as paneApi,
  pendingCommand,
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
  type UnsavedTab,
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
 * What closing one tab would cost, asked before the command rather than after it.
 *
 * Answers `{ unsaved: [], sessions: [] }` without an IPC call when the tab has no session
 * bound at all, which is every file tab that has not been split and every diff tab. In that
 * case the unsaved half is left to Rust's refusal, which already names the file — paying a
 * round trip to be told the same thing on every `×` is the kind of cost that gets a guard
 * deleted later.
 *
 * When there *is* a session bound the round trip happens anyway, and then **both** halves
 * come back from it. That matters: a file tab split to hold a Claude pane can be dirty and
 * mid-turn at the same time, and asking only about the session produced a dialog that said
 * nothing about the buffer and a "Close anyway" that discarded it. The user opted in to
 * losing a turn, not to losing their edits.
 *
 * The session ids come from the local mirror and the *states* from Rust, because only the
 * hook server knows which are mid-turn — and `app.quitRequested` already applies the
 * `confirmCloseWithLiveSession` setting, so the session half inherits it. The unsaved half
 * deliberately does not; see `Settings::confirm_close_with_live_session` in Rust.
 */
async function tabCloseRisk(
  boot: Bootstrap | null,
  project: ProjectId,
  tab: TabId,
): Promise<{ unsaved: UnsavedTab[]; sessions: SessionSummary[] }> {
  const nothing = { unsaved: [], sessions: [] }
  const target = boot?.workspace.projects[project]?.tabs.find((t) => t.id === tab)
  if (!target) return nothing
  const bound = new Set(
    Object.values(target.tree.panes)
      .map((pane) => pane.session)
      .filter((session): session is SessionId => session !== null),
  )
  if (bound.size === 0) return nothing

  const decision = await appApi.quitRequested(project)
  return {
    // Narrowed to this tab: the decision answers for the whole project, and listing a dirty
    // file from a tab that is not closing would be a dialog about work that is not at risk.
    unsaved: decision.unsaved.filter((u) => u.tab === tab),
    sessions: decision.blocking.filter((s) => bound.has(s.session)),
  }
}

/**
 * Which projects this window has asked Rust to index, and over which roots.
 *
 * Module scope rather than store state, for the reason `treeStore`'s `inFlight` is: nothing
 * renders from it, and putting it in the store would make every `fs.index` a state update
 * and so a re-render of the whole shell.
 */
let indexedProjects = new Map<string, string>()

/**
 * Point the Rust-side file index at whatever projects the snapshot says are open.
 *
 * This is the call nothing was making. `fs.index` is what fills the tree and the picker, and
 * every `fs_*` and `picker_query` handler answers `NoIndex` until it has run — so before
 * this existed, the explorer showed zero rows and Ctrl+P showed an empty list, for ever, in
 * a build where all fourteen handlers were registered and working.
 *
 * Driven from the snapshot rather than from `openProject`, and the difference is not
 * cosmetic. `project.open` is only one of the ways a project comes to be open in this
 * window: a workspace restored from disk at launch has projects nobody opened this session,
 * and a project opened in a *second* window arrives here only as `cide://workspace-changed`.
 * Hanging the call off `openProject` would have left both of those cases exactly as broken
 * as they were.
 *
 * `Explorer.tsx` was the alternative and it loses on the same argument: it is handed one
 * project — this window's active one — so a second project in the same window would go
 * unindexed until the user switched to it, a detached-pane window would index nothing, and
 * the picker would depend on the sidebar being mounted.
 *
 * Every window runs this against the same workspace, so `fs.index` is called once per window
 * per project. That is deliberate and handled on the Rust side rather than here: `fs.index`
 * over roots that have already been walked is a no-op that answers with the existing index's
 * status, whether the first walk is still running or finished long ago (`FsRegistry::claim`),
 * and `fs.close` is idempotent. Suppressing it here instead would mean deciding which window
 * "owns" a project, which nothing else in this app has to decide.
 *
 * The *finished* half of that guarantee is the one this window depends on and the one that
 * was missing: `indexedProjects` is module state in one webview, so a second window — and a
 * detached pane is a window — starts with an empty map and asks for every open project long
 * after the first window's walk is done. A re-walk there is not merely wasted work; the walk
 * begins by clearing the matcher and dropping the watcher, so it would empty the *first*
 * window's Ctrl+P and stop its file events. `cmd::fs::tests` pins both halves.
 */
function syncFileIndex(workspace: Workspace): void {
  const open: IndexTarget[] = Object.values(workspace.projects).map((project) => ({
    project: project.id,
    roots: project.roots.map((root) => root.path),
  }))
  const plan = planFileIndex(indexedProjects, open)
  indexedProjects = plan.known

  // Not awaited: the walk is seconds of work on a large repository and the whole design is
  // that the window keeps painting while it runs. `pendingCommand` is what keeps a build
  // without these handlers — or a project whose roots have gone — from raising an unhandled
  // rejection out of a snapshot handler.
  for (const project of plan.index) {
    void pendingCommand('fs_index', () => fsApi.index(project), null)
  }
  for (const project of plan.close) {
    void pendingCommand('fs_close', () => fsApi.close(project), false)
  }
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
  closePane: (project: ProjectId, tab: TabId, pane: PaneId, force?: boolean) => Promise<void>
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
  // Read back from the attribute `public/theme-boot.js` already wrote in <head>, rather
  // than named literally here. This value is live for the whole window-open — `hydrate` is
  // an IPC round trip — and `App.tsx` writes it straight to `documentElement.dataset.theme`
  // on mount. A literal here therefore *overwrites* what theme-boot resolved from `?theme=`
  // and repaints the window in the wrong palette until the bootstrap lands, which is the
  // exact flash theme-boot exists to remove. Hardcoding `'light'` instead would fix the
  // default case and leave dark-theme users flashing; only the attribute knows.
  theme: globalThis.document?.documentElement.dataset.theme === 'dark' ? 'dark' : 'light',

  hydrate: async () => {
    const boot = await appApi.getBootstrap()
    set({ boot, theme: boot.workspace.settings.theme })
    // After the state is set, not before: the explorer and the picker read the project from
    // the store, and an index that started against a project the window has not adopted yet
    // would race the components that are about to ask it questions.
    syncFileIndex(boot.workspace)
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
    // Mostly no pre-flight question here, unlike `closeProject`: the unsaved case comes back
    // from Rust as a refusal that already names the file, so asking first would be a second
    // round trip for an answer the failure path hands over anyway — and it would be the
    // answer to a slightly older workspace.
    //
    // Sessions are the exception, because Rust has no business refusing an interrupted turn,
    // and they are asked about only when this tab actually has one bound. `tabCloseRisk`
    // reports the unsaved files in the same breath: once that round trip is being made, a
    // dialog that mentions only the session would let "Close anyway" discard a buffer the
    // user was never shown.
    if (!force) {
      const risk = await tabCloseRisk(get().boot, project, tab)
      if (risk.sessions.length > 0 || risk.unsaved.length > 0) {
        requestCloseConfirm({
          scope: 'tab',
          unsaved: risk.unsaved,
          sessions: risk.sessions,
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
  closePane: async (project, tab, pane, force = false) => {
    // Three steps, and the order is the whole of it.
    //
    // 1. The domain call, which may be refused — the project console's primary pane cannot
    //    go, nor a tab's last, nor an editor pane holding unsaved edits — so nothing may be
    //    disposed until it has succeeded. The unsaved refusal parks the same dialog a tab
    //    close does; discarding re-issues with `force: true`.
    const parked = await refused(
      'pane',
      () => paneApi.close(project, tab, pane, force),
      () => get().closePane(project, tab, pane, true),
    )
    if (parked) return

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
    // A project opened or closed in *another* window reaches this one only here. Without
    // this line the second window's tree and picker stay empty until something in it happens
    // to call `hydrate`.
    syncFileIndex(workspace)
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
