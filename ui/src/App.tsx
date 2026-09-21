/**
 * The shell.
 *
 * Composes the chrome surfaces and wires them to the Rust-owned workspace. Every chrome
 * component is a pure render target that reads nothing from the store — the wiring lives
 * here, in one place, which is also what lets the layout audit drive each surface with
 * fixed props.
 *
 * The pane grid is still M0's hard-coded 2x2. M4 replaces it with the real split tree
 * rendered from `tab.tree`.
 */
import {
  memo,
  useCallback,
  useEffect,
  useInsertionEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from 'react'
import { AppHeader } from '@/chrome/AppHeader'
import { ActivityRail } from '@/chrome/ActivityRail'
import { PanelBoundary } from '@/chrome/PanelBoundary'
import {
  SIDEBAR_INITIAL,
  isPanelOpen,
  selectView,
  showPanel,
  toggleSidebar,
  type ActivityView,
  type SidebarState,
} from '@/chrome/sidebarView'
import { registerPanelHost, registerSettingsFrameHost } from '@/chrome/panelRequests'
import { StatusBar } from '@/chrome/StatusBar'
import {
  formatClaude,
  useSessionStatus,
  type StatusPayload,
} from '@/store/sessionStatus'
import { TabStrip } from '@/chrome/TabStrip'
import { WindowFrame } from '@/chrome/WindowFrame'
import { auditMode, formatAuditReport, runLayoutAudit } from '@/chrome/layoutAudit'
import { AUDIT_ACTIVE_TAB, AUDIT_GIT_CHANGES, AUDIT_TABS } from '@/chrome/auditFixture'
import { useGitChangeCount } from '@/chrome/gitCountStore'
import { auditPanesMode, formatPaneAudit, runPaneAudit } from '@/layout/paneAudit'
import { createAppPaneDriver } from '@/layout/appPaneDriver'
import { auditWindowsMode, formatWindowAudit, runWindowAudit } from '@/layout/windowAudit'
import { createAppWindowDriver } from '@/layout/appWindowDriver'
import { DetachedPaneWindow } from '@/windows/DetachedPaneWindow'
import { SplitTree } from '@/layout/SplitTree'
import { ToolWindowHost } from '@/toolwindow/ToolWindowHost'
import { TabContent } from '@/layout/TabContent'
import { PROJECT_NOTES } from '@/sidebar/groupRows'
import { Explorer } from '@/sidebar/Explorer'
import { GitPanel } from '@/sidebar/GitPanel'
import { SearchPanel } from '@/sidebar/SearchPanel'
import { ProblemsPanel } from '@/sidebar/ProblemsPanel'
import { ALL_VISIBLE, anyScanning, applyFilters, statusBarCounts } from '@/sidebar/ProblemsPanel/model'
import {
  isDiagnosticSourceId,
  refreshDiagnostics,
  restartDiagnosticSource,
} from '@/sidebar/ProblemsPanel/actions'
import { useDiagnostics } from '@/sidebar/diagnosticsStore'
import { AgentsPanel } from '@/sidebar/AgentsPanel'
// M22. Imported statically like every other panel: `panes/PaneBody.tsx` states the rule for panes
// and it holds here too — a panel that renders nothing for a frame while a chunk arrives is a
// panel the layout measures at zero.
import { ExtensionsPanel } from '@/sidebar/ExtensionsPanel'
import { useInstalledExtensions } from '@/ext/extStore'
import { ExtPanelHost } from '@/ext/ExtPanelHost'
import type { PanelBinding } from '@/ipc/client'
import { attachExtensions, useExtPanels } from '@/ext/extStore'
import { attachEditorBridge, setBridgeProject } from '@/ext/editorBridge'
import { ExtensionTab } from '@/ext/ExtensionTab'
/* Straight from `model.ts` and not through the barrel, per that file's own rule: the
   barrel pulls in React and the DOM, and `model.ts` is import-free so `check:agents` can
   compile it standalone. `ProblemsPanel/model` is imported the same way two lines up. */
import { liveCount } from '@/sidebar/AgentsPanel/model'
import { useAgents } from '@/sidebar/agentsStore'
import { TasksPanel, TaskDetailHost } from '@/sidebar/TasksPanel'
import { DockerFilesPane } from '@/panes/DockerFilesPane'
import { DockerInspectPane } from '@/panes/DockerInspectPane'
import { DocsPane } from '@/panes/DocsPane'
import { useDocker } from '@/sidebar/dockerStore'
import { OpenSpecPanel } from '@/sidebar/OpenSpecPanel'
import { ProposeDialog, useProposeDialog } from '@/sidebar/OpenSpecPanel/ProposeDialog'
import { SpecTab } from '@/sidebar/OpenSpecPanel/SpecTab'
import { useSpec } from '@/sidebar/specStore'
import { openCount } from '@/sidebar/TasksPanel/model'
import { useTasks } from '@/sidebar/tasksStore'
import { OverlayHost } from '@/overlays/OverlayHost'
import { closeOverlay, useOverlayOpen } from '@/overlays/store'
import { Failures } from '@/chrome/Failures'
import { notify, notifyFailure } from '@/chrome/notices'
import { Switcher } from '@/chrome/Switcher'
import { SidebarSplitter } from '@/chrome/SidebarSplitter'
import { installNativeMenuSuppression, useContextMenuOpen } from '@/menus'
import { TransportNotice } from '@/ipc/TransportNotice'
import { CloseConfirm } from '@/chrome/CloseConfirm'
import { useCloseConfirm, requestCloseConfirm } from '@/chrome/closeConfirmStore'
import { OutsideOpenGate } from '@/chrome/OutsideOpenGate'
import { LogDetailCard } from '@/chrome/LogDetailCard'
import { PullStrategyGate } from '@/chrome/PullStrategyGate'
import { FileProperties } from '@/chrome/FileProperties'
import { PushDialog } from '@/chrome/PushDialog'
import { ConflictsDialog } from '@/chrome/ConflictsDialog'
import { requestOutsideOpen } from '@/chrome/outsideOpenStore'
import { outsideAsk } from '@/terminal/outsideOpen'
import { canSaveAll, saveAll } from '@/editor/openBuffers'
import { revealPane } from '@/editor/revealPane'
import { jumpTo, pendingJump } from '@/editor/jump'
import { UNKNOWN_LINE } from '@/editor/navHistory'
import { claudeSend, dockerEvents, specEvents } from '@/ipc/client'
import { startFileDrop } from '@/sidebar/TasksPanel/fileDrop'
import { useKeyGate } from '@/keys/useKeyGate'
import { createDispatcher } from '@/keys/dispatch'
import { buildKeymap } from '@/keys/keymap'
import { PaneFrame } from '@/layout/PaneTitleBar'
import { PaneBody } from '@/panes/PaneBody'
import { GitDiffPane } from '@/panes/GitDiffPane'
import { changeNavPresent, subscribeChangeNav } from '@/panes/changeNav'
import { focusPaneDom } from '@/panes/paneFocus'
import { liveHosts } from '@/layout/paneHosts'
import { SettingsTab, lastFrameSection } from '@/settings/SettingsTab'
import { SettingsFrame } from '@/settings/SettingsFrame'
import { toggleTheme as togglePersistedTheme } from '@/settings/useSettings'
import {
  app as appApi,
  benchMode,
  file as fileApi,
  fs as fsApi,
  windows as windowApi,
  diag,
  events,
  settings as settingsApi,
  agentEvents,
  taskEvents,
  toolWindow as toolWindowApi,
  windowLabel,
  windowRole,
  type PaneRestore,
  type Project,
  type ProjectId,
  type SettingsSection,
  type SplitIntent,
  type Tab,
  type TabId,
  type WindowRole,
} from '@/ipc/client'
import { DetachedTabHeader } from '@/windows/DetachedTabHeader'
import { clusterPlan, shownProjects, shownTabs } from '@/windows/windowTabs'
import { runBench, formatReport, probeIpcOnce } from '@/bench/ipcBench'
import { retheme } from '@/terminal/xterm'
import { useWorkspace } from '@/store/workspace'
import { useFileTree } from '@/sidebar/treeStore'
import { focusedTabPath } from '@/keys/target'
import styles from './App.module.css'

/**
 * Wait until the browser has laid out and the webfonts have resolved.
 *
 * Two frames rather than one: the first lets React commit, the second lets layout run
 * against it. `fonts.ready` matters because an unloaded face is measured at the fallback
 * stack's metrics — the exact failure self-hosting exists to prevent, and one that would
 * otherwise show up as a passing audit.
 */
function settled(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        void document.fonts.ready.then(() => resolve())
      }),
    )
  })
}

/**
 * The repository the audits seed an empty workspace with.
 *
 * `__CIDE_REPO_ROOT__` is substituted by `vite.config.ts`, and only when Vite is *serving*
 * — a production build defines it as the empty string, so no machine's directory layout is
 * ever baked into `ui/dist`. That is the whole reason it is not a plain literal: this was
 * one author's absolute home path for six milestones, which made every audit a no-op on any
 * other clone and put that path in a published bundle.
 *
 * The audits are dev-only (`run.sh --audit-*`), so an empty value here is never reached by
 * one; `seedAuditWorkspace` refuses it rather than opening the process's cwd by accident.
 */
const AUDIT_PROJECT_ROOT = __CIDE_REPO_ROOT__

/**
 * Open this repo and a second Claude tab, if the workspace is empty.
 *
 * The pane audit needs a tree to split and two tabs to switch between; a run against an
 * empty workspace passes by doing nothing, which is the one result it must never give.
 */
async function seedAuditWorkspace(
  openProject: (paths: string[]) => Promise<void>,
  newClaudeTab: (project: ProjectId) => Promise<TabId>,
): Promise<void> {
  // The guard must be on `boot` being *present*, not on the project count.
  //
  // `boot?.workspace.projects ?? {}` reads as "no projects" while bootstrap is still in
  // flight, which is exactly when this runs. Every audit run therefore seeded another copy
  // of the same directory: a development workspace reached 242 projects this way, and in
  // per-project window mode the next launch tried to open a window for each one.
  //
  // The domain now refuses a duplicate path outright — `open_project` activates the existing
  // project instead — so this is belt and braces. Both are worth having: one stops the
  // symptom here, the other stops it for every caller.
  const boot = useWorkspace.getState().boot
  if (boot === null) return

  if (Object.keys(boot.workspace.projects).length === 0) {
    if (!AUDIT_PROJECT_ROOT) return
    await openProject([AUDIT_PROJECT_ROOT])
  }
  const project = activeProjectIdOfState()
  if (!project) return
  const tabs = useWorkspace.getState().boot?.workspace.projects[project]?.tabs ?? []
  if (tabs.length < 2) await newClaudeTab(project)
}

/** The active project of whatever the store currently holds, outside a render. */
function activeProjectIdOfState() {
  const boot = useWorkspace.getState().boot
  if (!boot || boot.role.kind !== 'shell') return null
  return boot.role.active
}

/**
 * Give the windows audit something a detach can legally take. Audit-only.
 *
 * `seedAuditWorkspace` produces two tabs of one pane each, and the domain refuses to
 * detach both of them: `take_pane` refuses a tab's last pane, and the console's founding
 * pane is `Primary` and refused outright. So the audit's detach criterion needs an
 * *auxiliary* pane in a *non-console* tab — which is exactly what its own failure message
 * asks for ("seed a tab with a second pane before running"). A shell rather than a Claude
 * session, so a hundred audit runs do not spend the user's quota starting agents.
 */
async function seedDetachablePane(
  splitPane: ReturnType<typeof useWorkspace.getState>['splitPane'],
): Promise<void> {
  const boot = useWorkspace.getState().boot
  const project = activeProjectIdOfState()
  if (!boot || !project) return
  const tabs = boot.workspace.projects[project]?.tabs ?? []
  const tab = tabs.find((t, at) => at > 0 && Object.keys(t.tree.panes).length === 1)
  if (!tab) return
  await splitPane(project, tab.id, tab.tree.focused, 'row', 'after', { kind: 'shell' })
}

/**
 * Resolve once the bootstrap has landed. Audit-only.
 *
 * The pane and window audits run from mount effects, and a mount effect always beats the
 * bootstrap's IPC round trip — so `seedAuditWorkspace`'s deliberate bootless refusal made
 * both audits silently no-op ("no project to run against") on every launch where the race
 * fell that way. The chrome audit re-runs on `[boot]`; these two are one-shots, so they
 * wait here instead of re-running on every later snapshot.
 */
async function bootLanded(cancelled: () => boolean): Promise<boolean> {
  while (!cancelled()) {
    if (useWorkspace.getState().boot !== null) return true
    await new Promise((resolve) => setTimeout(resolve, 50))
  }
  return false
}

/** The viewport the design mock is drawn at, and the one the audit must measure in. */
const MOCK_WIDTH = 1440
const MOCK_HEIGHT = 900

/**
 * Stable empties for the memoised derivations below, so a bootless window (and every render
 * before the first snapshot) hands React the same array identity every time — the rule
 * `useProjects`' `NO_PROJECTS` writes down in the store. `never[]` so each site keeps the
 * element type its non-empty arm derives.
 */
const NO_PROJECT_LIST: never[] = []
const NO_TAB_LIST: never[] = []

// WebKitGTK draws its own menu on every right-click, and with `devtools` enabled that menu
// offers "Inspect element". Suppressed here rather than per surface: a surface-by-surface
// `preventDefault` is correct only for the surfaces someone remembered, so the next pane
// added leaks it again. Module scope because it is window-lifetime — StrictMode's double
// mount cannot install it twice, and no unmount can tear it down while the window is open.
//
// Text inputs keep the native menu deliberately: WebKit refuses `execCommand('paste')` from
// page script, so a hand-rolled Paste item would be drawn and then do nothing. See
// `ui/src/menus` for how a surface opts out of that.
installNativeMenuSuppression()

export function App() {
  const boot = useWorkspace((s) => s.boot)
  const theme = useWorkspace((s) => s.theme)
  const hydrate = useWorkspace((s) => s.hydrate)
  const openProject = useWorkspace((s) => s.openProject)
  const activateProject = useWorkspace((s) => s.activateProject)
  const closeProject = useWorkspace((s) => s.closeProject)
  // The strip and grid actions moved to `WorkspaceContent` below, which reads them itself;
  // what stays here is what App's own chrome — the header, the dispatcher fallback, the
  // detached-pane branch — still dispatches.
  const splitPane = useWorkspace((st) => st.splitPane)
  const contextMenuOpen = useContextMenuOpen()
  /*
   * Whether a change-walkable diff surface holds the slot, for the `diffFocused` key-context
   * flag below.
   *
   * The snapshot is a **boolean** and must stay one. `useSyncExternalStore` compares snapshots
   * with `Object.is`, so a getter returning a fresh `{ index, count }` would never compare equal
   * and the re-render loop that followed would end at *Maximum update depth exceeded* with the
   * whole root unmounted — which is the failure `check:selectors` exists for, arriving through a
   * different hook. The server snapshot is `false`: a detached window's first paint has nothing
   * mounted yet, and claiming otherwise would arm a chord against a pane that is not there.
   */
  const diffFocused = useSyncExternalStore(subscribeChangeNav, changeNavPresent, () => false)
  const bindSession = useWorkspace((st) => st.bindSession)
  const newClaudeTab = useWorkspace((st) => st.newClaudeTab)
  const detachPane = useWorkspace((st) => st.detachPane)
  const redockPane = useWorkspace((st) => st.redockPane)
  const redockTab = useWorkspace((st) => st.redockTab)

  /*
   * Whether this webview is a `tab:<uuid>` window — one torn-out tab and nothing else.
   *
   * From the URL rather than from `boot.role`, for the reason `PaneTitleBar` reads its
   * window kind there: it is true for the whole life of the window and known before the
   * first bootstrap round trip resolves. The sidebar's initial state depends on it *at
   * mount* — a `useState` initialiser runs once, and deciding from a `boot` that is still
   * `null` would flash the Files panel into a window that has no rail to ever close it.
   */
  const tabWindow = windowRole() === 'tab'

  /**
   * Which sidebar view the rail has lit, whether a panel is showing, and which one comes back.
   *
   * `view: null` is a real state and always was: clicking the lit rail button hides its panel,
   * so the workspace can have the full window width without dragging the splitter to the edge.
   * What it had no way to be was *reached without a mouse*, and no memory of what to restore —
   * see `chrome/sidebarView.ts`, which owns every rule about this value so that a check script
   * can compile them. Nothing here is persisted; that module says why.
   */
  // Closed from the first render in a torn-out tab window, which draws no rail: the
  // initialiser runs once, and `SIDEBAR_INITIAL` there would mount a Files panel with no
  // button anywhere that could ever dismiss it.
  const [sidebar, setSidebar] = useState<SidebarState>(
    tabWindow ? { view: null, last: SIDEBAR_INITIAL.last } : SIDEBAR_INITIAL,
  )
  /*
   * This window's answer to "reveal a panel", for every caller that is not inside React.
   *
   * `setSidebar` used to reach the dispatcher as a `showSidebar` prop and reach nothing else, so
   * a gesture made anywhere but a `keys/dispatch.ts` arm could not open a panel at all — which
   * is why the git log's *Amend…*, whose entire job is to put the user in front of the commit
   * box, shipped listed and disabled. `chrome/panelRequests.ts` holds the slot; this is the one
   * registration that fills it, and every reveal in the app now ends here.
   *
   * **Not registered in a detached window of either kind.** The pane window returns long before
   * the sidebar is rendered, and the tab window renders the shell path with no rail and no
   * panels — so a registration in either would accept requests, move a `useState` nothing
   * draws, and report success — a silent no-op in place of the `unmet` line the dispatcher
   * writes when the slot is empty. The refusal is the whole value of the slot being fillable.
   *
   * `setSidebar` is a `useState` setter and therefore stable, so this runs once per window. The
   * cleanup is not optional: a root that goes away while registered leaves `requestPanel` calling
   * into a dead React tree.
   */
  const paneWindow = boot?.role.kind === 'detachedPane'
  useEffect(() => {
    if (paneWindow || tabWindow) return
    registerPanelHost((view) => setSidebar((s) => showPanel(s, view)))
    return () => registerPanelHost(null)
  }, [paneWindow, tabWindow])
  /**
   * Which overlay is up, if any. `null` is the ordinary state.
   *
   * Read from `@/overlays/store`, which is the only writer. A `useState` here was a second
   * source of truth for one value, and the two disagreed in the direction that matters: the
   * key gate dispatches `picker.files` from outside React and writes the *store*, so Ctrl+P
   * set a flag this component never read and the host below was never mounted. The overlay
   * was unreachable by the gesture it exists for.
   */
  const overlay = useOverlayOpen()
  /*
   * A close that Rust refused because it would discard unsaved work.
   *
   * The store parks the refusal here and re-issues the command with `force: true` if the
   * user chooses to discard, so no call site needs to know this dialog exists. The dialog
   * is the courtesy; `CoreError::UnsavedChanges` is the enforcement — a dialog that fails
   * to render fails open, and a buffer would be gone.
   */
  const pendingClose = useCloseConfirm((s) => s.pending)
  /**
   * The restore plan, keyed by pane, and the projects it has been asked for.
   *
   * Read once **per project** and never refreshed for that project: an entry describes what a
   * pane's session was when its project appeared in this window, so a pane created later has
   * no entry and spawns immediately — which is correct, because the user just asked for it.
   *
   * It used to be read once per *window*, at boot, which was the same thing while every
   * project a window would ever show was in the workspace at boot. It stopped being the same
   * thing when a closed project started keeping its layout (`persist::closed.json`): a project
   * reopened during the run arrives with panes holding conversations from before the close,
   * and with no entry those panes spawned fresh — "claude sessions aren't restored". So the
   * effect below watches the snapshot's project set and asks Rust for the plan of every project
   * it has not planned yet, at boot and whenever one appears.
   *
   * **A project's panes are not rendered until its plan is in** — `plannedProjects` is the
   * gate, in both window kinds. The plan used to start as an empty map, which is
   * indistinguishable from "the plan says nothing about any of these panes" — and that is a
   * different claim with two consequences, both silent. `PaneBody` latches its Resume splash on
   * the *first* render (the splash and the terminal are different element types in one
   * position, so it cannot be recomputed), so a tree painted before the plan arrived spawned
   * every restored Claude pane at once — the "reopening a six-pane project starts six agents"
   * case the splash exists to prevent. And a pane with no entry never passes `--resume`, so the
   * conversation comes back empty. Both depended on two unrelated effects resolving in the
   * order they were declared, which the per-project gate no longer assumes.
   *
   * A failed fetch marks the project planned all the same: a plan that cannot be read costs a
   * pane its resume, and a project that never renders costs the user everything.
   */
  const [restorePlan, setRestorePlan] = useState<ReadonlyMap<string, PaneRestore>>(
    () => new Map(),
  )
  const [plannedProjects, setPlannedProjects] = useState<ReadonlySet<string>>(() => new Set())
  // Projects a plan has been *requested* for, so StrictMode's double effect and a snapshot
  // arriving mid-fetch cannot ask twice. A ref: it is bookkeeping for the effect, not a render.
  const planRequested = useRef<Set<string>>(new Set())
  // The snapshot's project map — a stable object per accepted mutation, which is what makes it
  // a legal selector and a usable dependency; the ids are read inside the effect.
  const openProjects = useWorkspace((s) => s.boot?.workspace.projects)
  const [bench, setBench] = useState<string | null>(null)
  const [benchRunning, setBenchRunning] = useState(false)

  useEffect(() => {
    document.documentElement.dataset.theme = theme
    // Terminals hold a resolved colour table rather than CSS variables, so they need
    // repainting explicitly when the tokens change.
    retheme([...liveHosts()].flatMap((h) => (h.terminal ? [h.terminal] : [])))
  }, [theme])

  useEffect(() => {
    // Follow every mutation, not only this window's own. Two windows now edit one tree.
    let unlisten: (() => void) | null = null
    void useWorkspace
      .getState()
      .subscribe()
      .then((fn) => {
        unlisten = fn
      })
    return () => unlisten?.()
  }, [])

  // Live token and cost figures. Subscribed once per window; the bar below picks whichever
  // session the focused pane holds.
  useEffect(() => {
    let unlisten: (() => void) | null = null
    void events
      .onSessionStatus((session, status) => {
        useSessionStatus.getState().set(session, status as StatusPayload)
      })
      .then((fn) => {
        unlisten = fn
      })
    return () => unlisten?.()
  }, [])

  // A window-manager close the Rust side refused. It raises the same dialog the command
  // paths raise; discarding re-issues `window_close` with `force: true`, which does have a
  // command behind it.
  useEffect(() => {
    let unlisten: (() => void) | null = null
    void events
      .onCloseBlocked((label, unsaved) => {
        requestCloseConfirm({
          scope: 'window',
          unsaved,
          sessions: [],
          proceed: async () => {
            await windowApi.close(label, true)
          },
        })
      })
      .then((fn) => {
        unlisten = fn
      })
    return () => unlisten?.()
  }, [])

  useEffect(() => {
    if (!openProjects) return
    for (const id of Object.keys(openProjects)) {
      if (planRequested.current.has(id)) continue
      planRequested.current.add(id)
      void appApi
        .restorePlan(id)
        .then((plan) => {
          setRestorePlan((prev) => {
            const next = new Map(prev)
            for (const entry of plan) next.set(entry.pane, entry)
            return next
          })
        })
        .catch((e) => {
          // Planned without entries: every pane of this project then spawns fresh, which is
          // the same thing this window did before the plan existed. Leaving the project
          // unplanned would hold it off the screen because one optimisation could not be read.
          return diag.log(`restore plan unavailable for project ${id}: ${String(e)}`)
        })
        .finally(() => setPlannedProjects((prev) => new Set(prev).add(id)))
    }
  }, [openProjects])

  useEffect(() => {
    void appApi.ready()
    void hydrate()
      .then(async () => {
        // The chrome audit needs a project tab and a tab strip, neither of which exists in
        // an empty workspace. The pane audit seeds itself, in sequence — see below.
        if (!auditMode()) return
        if (Object.keys(useWorkspace.getState().boot?.workspace.projects ?? {}).length > 0) return
        if (!AUDIT_PROJECT_ROOT) return
        await openProject([AUDIT_PROJECT_ROOT])
      })
      .catch((e) => diag.log(`bootstrap failed: ${String(e)}`))
  }, [hydrate, openProject])

  useEffect(() => {
    if (!auditMode()) return
    let cancelled = false

    // The milestone asks for both themes. Geometry should be theme-independent — every
    // dimension comes from a token that only carries colour — but "should be" is the claim
    // under test, and a font or border that differs between palettes would move a box.
    // So the audit sweeps: measure, switch, measure, and restore.
    void (async () => {
      // The mock states 1440x900, and every dimension below is compared against numbers
      // taken from it. Reporting a measurement made at some other viewport as though it
      // were that one is exactly the quiet inaccuracy this audit exists to catch.
      for (let attempt = 0; attempt < 3; attempt++) {
        if (window.innerWidth === MOCK_WIDTH && window.innerHeight === MOCK_HEIGHT) break
        await appApi.setViewport(MOCK_WIDTH, MOCK_HEIGHT)
        await settled()
      }

      const initial = useWorkspace.getState().theme
      for (const next of ['dark', 'light'] as const) {
        if (cancelled) return
        useWorkspace.getState().setTheme(next)
        await settled()
        if (cancelled) return
        await diag.log(formatAuditReport(runLayoutAudit()))
      }
      useWorkspace.getState().setTheme(initial)
    })()

    return () => {
      cancelled = true
    }
  }, [boot])

  useEffect(() => {
    if (!auditPanesMode()) return
    let cancelled = false

    // Sequenced after seeding rather than polling for a driver. Polling raced the second
    // tab into existence mid-run: the audit split a pane in the tab that was active when it
    // started, `newClaudeTab` then made a different tab active, and the audit waited two
    // seconds for a pane that had landed somewhere it was no longer looking.
    void (async () => {
      // …and the seeding after the bootstrap, or it refuses to seed at all — see
      // `bootLanded` for the race that made this audit a silent no-op.
      if (!(await bootLanded(() => cancelled))) return
      await seedAuditWorkspace(openProject, newClaudeTab)
      if (cancelled) return
      const driver = createAppPaneDriver()
      if (!driver) {
        void diag.log('pane audit: no project to run against')
        return
      }
      const result = await runPaneAudit(driver)
      if (!cancelled) void diag.log(formatPaneAudit(result))
    })().catch((e) => diag.log(`pane audit failed: ${String(e)}`))

    return () => {
      cancelled = true
    }
  }, [openProject, newClaudeTab])

  useEffect(() => {
    if (!auditWindowsMode()) return
    let cancelled = false

    void (async () => {
      // Two projects, so a mode flip has something to distribute, and a second tab so the
      // console's pane is not the only one — detaching a tab's last pane is refused.
      // Seeded only after the bootstrap, for the pane audit's reason (`bootLanded`), and
      // with one auxiliary pane the domain will actually let a detach take.
      if (!(await bootLanded(() => cancelled))) return
      await seedAuditWorkspace(openProject, newClaudeTab)
      if (cancelled) return
      await seedDetachablePane(splitPane)
      if (cancelled) return
      const driver = createAppWindowDriver()
      if (!driver) {
        void diag.log('window audit: no project to run against')
        return
      }
      const result = await runWindowAudit(driver)
      if (!cancelled) void diag.log(formatWindowAudit(result))
    })().catch((e) => diag.log(`window audit failed: ${String(e)}`))

    return () => {
      cancelled = true
    }
  }, [openProject, newClaudeTab])

  /*
   * The IPC health probe, on every boot rather than only under `CIDE_BENCH=1`.
   *
   * WebKitGTK can fall back from the custom protocol to string `postMessage` with no error
   * and no Rust-side signal. The only symptom is a terminal that feels inexplicably sluggish,
   * because every PTY frame is now a JSON array of decimal numbers spelled into a
   * `webview.eval`. Guarded behind `benchMode()` this ran only during a benchmark — which is
   * to say, never on the boots that could actually be degraded.
   *
   * Cheap, once per window, and it cannot throw: `probeIpcOnce` swallows its own failures
   * into a `diag.log` line, because a diagnostic that can break a boot is worse than no
   * diagnostic. Per window on purpose — each webview negotiates its own transport.
   */
  useEffect(() => {
    void probeIpcOnce()
  }, [])

  useEffect(() => {
    if (!benchMode()) return
    void runBench()
      .then((r) => diag.benchReport(formatReport(r)))
      .catch((e) => diag.benchReport(`benchmark failed: ${String(e)}`))
  }, [])

  const onBench = useCallback(async () => {
    setBenchRunning(true)
    setBench('running…')
    try {
      setBench(formatReport(await runBench()))
    } catch (e) {
      setBench(`benchmark failed: ${String(e)}`)
    } finally {
      setBenchRunning(false)
    }
  }, [])

  /*
   * The projects THIS window's header draws — `windows/windowTabs.ts` holds the rule and says
   * why every project in the process is the wrong answer: in `PerProject` mode each window's
   * role names exactly one, and rendering the other two put tabs in the strip that activate a
   * project this window does not show.
   *
   * Memoised on `boot` (this one and `visibleTabs` below): a snapshot replaces `boot`, so
   * these still recompute exactly once per mutation — what the memo removes is the fresh
   * array per *non-snapshot* render (an overlay opening, a statusline tick), which is what
   * lets `WorkspaceContent`'s memo below actually hold.
   */
  const projects = useMemo(
    () =>
      boot ? shownProjects(boot.role, Object.values(boot.workspace.projects)) : NO_PROJECT_LIST,
    [boot],
  )
  // A detached pane or tab window shows one project too, and naming it here keeps the
  // header honest in those windows rather than rendering no active tab at all.
  const activeProjectId =
    boot === null
      ? null
      : boot.role.kind === 'shell'
        ? boot.role.active
        : boot.role.project
  const activeProject = activeProjectId ? (boot?.workspace.projects[activeProjectId] ?? null) : null

  /*
   * Whether the empty frame is showing the Settings screen. (M74)
   *
   * Webview state, and one of the two kinds `store/workspace.ts`'s header says the webview
   * legitimately owns: which surface is up, for the length of a session. Rust could not hold it
   * even if it should — a Settings *tab* is a tab of a project, which is the whole reason this
   * exists (see `settings/SettingsFrame.tsx`) — and a relaunch with nothing open is a fresh
   * start, so there is nothing to persist.
   *
   * The render is gated on `activeProject === null` as well, so opening a project reveals the
   * project rather than the settings screen; the effect below clears the flag behind that gate,
   * or closing the last project again would bring back a screen nobody asked for a second time.
   */
  const [settingsFrame, setSettingsFrame] = useState<SettingsSection | null>(null)
  useEffect(() => {
    if (activeProjectId !== null) setSettingsFrame(null)
  }, [activeProjectId])
  // Stable, because `WorkspaceContent` is memoised and a fresh closure per render would defeat
  // that for every surface it draws — the reason `runCommand` beside it is a `useCallback` too.
  const closeSettingsFrame = useCallback(() => setSettingsFrame(null), [])
  /*
   * The seam `keys/dispatch.ts` reaches this through — `settings.open` and `settings.keymap`
   * are not inside React, which is the whole of what `registerSettingsFrameHost` is for.
   *
   * `null` means *wherever it was left*, read from `SettingsTab`'s own memory. That is why a
   * plain ⚙ or `settings.open` while the screen is already up changes no state and therefore
   * remounts nothing: the value it sets is the value already there. `settings.keymap` does set
   * a different one, and the remount that follows is the screen opening on the section that was
   * asked for — see `SettingsFrame`'s `key`.
   */
  useEffect(() => {
    if (boot?.role.kind !== 'shell') return
    registerSettingsFrameHost((section) =>
      setSettingsFrame((section as SettingsSection | null) ?? lastFrameSection()),
    )
    return () => registerSettingsFrameHost(null)
  }, [boot?.role.kind])

  /*
   * The tabs THIS window draws — `windows/windowTabs.ts` holds the rule and says why it is
   * load-bearing: a torn-out tab stays in `project.tabs` (so quit guards, restore plans and
   * awaiting counts keep seeing it), and this filter is the only thing standing between one
   * file tab and two live buffers over one file. A shell subtracts the torn-out tabs; a
   * `tab:` window keeps exactly its own.
   */
  const visibleTabs = useMemo(
    () =>
      boot !== null && activeProject !== null
        ? shownTabs(boot.role, activeProject.tabs, boot.workspace.windows)
        : NO_TAB_LIST,
    [boot, activeProject],
  )
  const tabRole = boot !== null && boot.role.kind === 'detachedTab' ? boot.role : null
  // The tab a `tab:` window shows — the role's, never the project's `activeTab`, which the
  // domain deliberately keeps pointed at a tab the *shell* draws. `null` while the mirror
  // has not caught up, or after the tab or its project closed under the window.
  const windowTab =
    tabRole !== null && activeProject !== null
      ? (activeProject.tabs.find((t) => t.id === tabRole.tab) ?? null)
      : null

  /*
   * The activity rail's changed-file count.
   *
   * Subscribed here rather than inside `ActivityRail` because every component in `chrome/` is
   * a pure render target that reads nothing from a store — that is the property
   * `chrome/auditFixture.ts` depends on, and the rail would have been the first exception.
   *
   * `null` for a detached-pane window, which returns below without ever rendering a rail:
   * walking its repositories would produce a number nothing draws. Passed as a value rather
   * than skipping the call, because the rule React enforces is that the hook order never
   * changes — not that hooks are only called when they are useful.
   */
  const gitChanged = useGitChangeCount(
    boot?.role.kind === 'detachedPane' ? null : activeProjectId,
  )

  /*
   * Every path the file tree can hang a row from, for the status bar's clickable path trail.
   * (M16)
   *
   * Subscribed here rather than inside `StatusBar` because every component in `chrome/` is a
   * pure render target that reads nothing from a store — the property `chrome/auditFixture.ts`
   * depends on. It moves twice in a session (the project's roots, and *External Libraries*
   * resolving), so a subscription rather than a one-off read; the store re-asks Rust on the
   * `cide://fs-status` the resolver emits, and this re-renders the bar when the answer grows.
   */
  const revealRoots = useFileTree((s) => s.revealable)

  /*
   * Diagnostics: one snapshot, filtered once, read by three surfaces. (M12)
   *
   * The filter is applied *here* and not in the store, and not in any of the three consumers.
   * `ProblemsPanel/model.ts` states the rule it follows: the panel and the status bar are two
   * renderings of one snapshot precisely so they cannot drift into disagreeing about whether the
   * workspace is clean. A second surface applying the filter itself is a second chance to
   * disagree; the rail badge makes it a third.
   */
  const rawDiagnostics = useDiagnostics((s) => s.snapshot)
  const attachDiagnostics = useDiagnostics((s) => s.attach)
  const scheduleDiagnostics = useDiagnostics((s) => s.schedule)
  const inspections = boot?.workspace.settings.inspections
  const diagnosticFilters = useMemo(
    () =>
      inspections === undefined
        ? ALL_VISIBLE
        : {
            severities: {
              error: inspections.severities.error,
              warning: inspections.severities.warning,
              // IDEA's "weak warning" is LSP's Information is our `info`. One thing, three names.
              info: inspections.severities.weakWarning,
              hint: inspections.severities.hint,
            },
            sources: inspections.sources,
            /*
             * **`all`, always** — the highlighting level is deliberately not applied here.
             *
             * It is a *per-editor* reading mode, and this filter feeds the Problems panel, the
             * status bar counts and the rail badge, which are workspace-wide. Putting it here
             * meant a user who set the default level to `None` got `✗ 0 ⚠ 0` in the status bar
             * and an empty panel over a workspace full of errors — a confident zero, which is the
             * one failure this whole surface is built to prevent. The level is applied once, in
             * `EditorPane`, to the buffer it belongs to.
             *
             * The mapping from the settings enum to the editor's vocabulary now lives in
             * `EditorPane`, which is the only layer that applies it.
             */
            level: 'all' as const,
          },
    [inspections],
  )
  const diagnostics = useMemo(
    () => applyFilters(rawDiagnostics, diagnosticFilters),
    [rawDiagnostics, diagnosticFilters],
  )
  // The rail's ⚠ badge, and since M28 its only consumer: the status bar's `✗ n ⚠ n` pair was
  // a second rendering of this figure one row down and went with the diff counters beside it.
  // Still a memo — `statusBarCounts` builds a fresh object, so an inline call would hand the
  // rail a new identity on every render of the shell.
  const diagCounts = useMemo(() => statusBarCounts(diagnostics.snapshot), [diagnostics])

  useEffect(() => {
    void attachDiagnostics(boot?.role.kind === 'detachedPane' ? null : (activeProjectId ?? null))
  }, [attachDiagnostics, activeProjectId, boot?.role.kind])

  useEffect(() => {
    // Only `cide://diagnostics`. Not `fs-changed`: a diagnostics push is authoritative, and
    // re-asking on every filesystem burst would double the work for no new information.
    const off = events.onDiagnostics((project) => {
      if (project === activeProjectId) scheduleDiagnostics()
    })
    return () => {
      void off.then((stop) => stop())
    }
  }, [activeProjectId, scheduleDiagnostics])

  /*
   * The task tracker: one board, read by the panel and by the rail's ☑ badge. (M18)
   *
   * Attached and subscribed **here** rather than inside `TasksPanel`, and that is the same rule
   * `gitCountStore` and the diagnostics pair above follow: the badge has to stay live while the
   * sidebar is shut or showing Files, and a listener registered inside a panel goes stale the
   * moment that panel unmounts. A user who closes the sidebar would otherwise see the count
   * freeze at whatever it was when they closed it — a number that looks current and is not.
   *
   * `null` in a detached-pane window, exactly as diagnostics is: that window has no rail, no
   * sidebar and no panel, so attaching would read a file for a surface nobody is looking at.
   */
  const taskBoard = useTasks((s) => s.board)
  const attachTasks = useTasks((s) => s.attach)
  const adoptTasks = useTasks((s) => s.adopt)
  /*
   * Which task's card is open — the gate on `TaskDetailHost`'s mount below. Subscribed here,
   * where `overlay` is, because the mount has to be conditional for its `PanelBoundary`'s sake:
   * the boundary never resets itself, so its Close must *unmount* it, and clearing the
   * selection is what collapses this branch. Selection changes are user gestures at overlay
   * open/close frequency, so the re-render this costs App is the one `useOverlayOpen` already
   * pays.
   */
  const taskSelected = useTasks((s) => s.selected)

  useEffect(() => {
    void attachTasks(boot?.role.kind === 'detachedPane' ? null : (activeProjectId ?? null))
  }, [attachTasks, activeProjectId, boot?.role.kind])

  useEffect(() => {
    /*
     * The payload carries the **whole board**, so this adopts it rather than scheduling a
     * re-ask: there is no round trip to coalesce and nothing to be gained by asking Rust for a
     * board it has just handed over. `.cide/tasks.json` has several writers — this window,
     * another window, and every dispatched agent — so snapshots can arrive out of order;
     * `tasksStore.adopt` runs `newerBoard`, which drops anything not strictly newer.
     *
     * The project is checked inside `adopt` as well, because every window hears every emit.
     */
    const off = taskEvents.onChanged((project, board) => {
      adoptTasks(project, board)
    })
    return () => {
      void off.then((stop) => stop())
    }
  }, [adoptTasks])

  /*
   * Files dragged in from the desktop, once per window. (M39) Here rather than in the task
   * card's host because the New task dialog is not under that host, and one subscription that
   * hit-tests the DOM at drop time serves every zone — see `TasksPanel/fileDrop.ts`.
   */
  useEffect(() => {
    const off = startFileDrop()
    return () => {
      void off.then((stop) => stop())
    }
  }, [])

  /*
   * OpenSpec: one board per project. (M28)
   *
   * Attached and subscribed here for the two blocks above's reason, with a third of its own: a
   * board read costs **two subprocesses**, so the event's payload is adopted directly and Rust is
   * never asked anything in response to it. Breaking that rule here would cost a Node process
   * every time an agent ticks a box.
   */
  const attachSpec = useSpec((s) => s.attach)
  const adoptSpec = useSpec((s) => s.adopt)
  const adoptDocker = useDocker((s) => s.adopt)

  useEffect(() => {
    attachSpec(boot?.role.kind === 'detachedPane' ? null : (activeProjectId ?? null))
  }, [attachSpec, activeProjectId, boot?.role.kind])

  useEffect(() => {
    const off = specEvents.onChanged((project, board) => {
      adoptSpec(project, board)
    })
    return () => {
      void off.then((stop) => stop())
    }
  }, [adoptSpec])

  /*
   * Docker: one board for the machine, read by the panel and by the rail's badge. (M41)
   *
   * Subscribed **here** rather than inside `DockerPanel`, for the reason the two blocks above
   * give: the badge has to stay live while the sidebar is shut or showing Files, and a listener
   * registered inside a panel goes stale the moment that panel unmounts.
   *
   * There is no `attach` beside it, unlike the spec board, and that is the whole difference a
   * machine-scoped board makes: no project to follow, and nothing to reset when one changes.
   * The first *read* is the panel's own, in `DockerPanelHost` — a daemon nobody has opened the
   * panel for is a socket cide has no reason to connect to.
   */
  useEffect(() => {
    const off = dockerEvents.onChanged((board) => {
      adoptDocker(board)
    })
    return () => {
      void off.then((stop) => stop())
    }
  }, [adoptDocker])

  /*
   * Subagents: one roster, read by the panel and by the rail's ⌬ badge. (M18)
   *
   * Attached and subscribed **here** rather than inside `AgentsPanel`, for the reason the block
   * above and `gitCountStore` both give: the badge has to stay live while the sidebar is shut or
   * showing Files, and a listener registered inside a panel goes stale the moment that panel
   * unmounts. A user who closes the sidebar would otherwise watch the run count freeze at
   * whatever it was when they closed it — a number that looks current and is not.
   *
   * `null` in a detached-pane window, exactly as diagnostics and tasks are: that window has no
   * rail, no sidebar and no panel, so attaching would read a project's config for a surface
   * nobody is looking at.
   */
  const agentRoster = useAgents((s) => s.roster)
  const attachAgents = useAgents((s) => s.attach)
  const adoptAgents = useAgents((s) => s.adopt)

  useEffect(() => {
    void attachAgents(boot?.role.kind === 'detachedPane' ? null : (activeProjectId ?? null))
  }, [attachAgents, activeProjectId, boot?.role.kind])

  useEffect(() => {
    /*
     * The payload carries the **whole roster**, so this adopts it rather than scheduling a
     * re-ask: there is no round trip to coalesce and nothing to be gained by asking Rust for a
     * roster it has just handed over. Unlike `cide://tasks-changed` there is no `rev` and none
     * is needed — the registry is one in-process writer, so the last emit is the newest by
     * construction; `agentsStore`'s header argues it.
     *
     * The project is checked inside `adopt` as well, because every window hears every emit.
     */
    const off = agentEvents.onChanged((project, roster) => {
      adoptAgents(project, roster)
    })
    return () => {
      void off.then((stop) => stop())
    }
  }, [adoptAgents])

  /*
   * The extension registry, attached here and not in the panel. (M22)
   *
   * The same rule the agents and tasks stores follow — a rail badge must stay live while the
   * sidebar is shut — with a stronger version of it: the *workers* are started by this
   * subscription, and an extension that only ran while somebody was looking at its panel could
   * never contribute a diagnostic or an outline. It is also not per project: an extension is a
   * tool the user chose, not a fact about a repository, which `cide_ext::config` argues at length.
   */
  const extPanels = useExtPanels()
  useEffect(() => attachExtensions(), [])

  /*
   * The rail's contributed buttons, in registry order. (M22) Derived here rather than inline
   * in the JSX so a render that changed nothing about extensions hands `ActivityRail` the
   * same array identity — the JSX spelling built a fresh array per render, which is exactly
   * the class of prop the memo boundary below cannot see through. The comment on *what* the
   * filter means stays at the JSX site.
   */
  const installedExts = useInstalledExtensions()
  /*
   * Which extensions the user has told the rail to leave out. (M31)
   *
   * > *"i doesn't like to stuck there all extensions (especially language like YAML, Proto,
   * > etc)"*
   *
   * A `Set` built once per registry change rather than a `find` inside the filter below, which
   * would be quadratic over two lists that are both "everything installed".
   *
   * This is *not* the answer to the short-window problem — `chrome/railOverflow.ts` is, and it
   * has to be: a rail that dropped buttons to fit would be unreachable panels with a setting to
   * blame. This is the answer to a different sentence in the same report, and the two are
   * independent. Nor is it `enabled`: hiding the button leaves the worker running, the language
   * highlighted and the diagnostics flowing, which is the whole point for a language extension.
   */
  const railHidden = useMemo(
    () =>
      new Set(
        installedExts
          .filter((ext) => !ext.railIcon)
          .map((ext) => `${ext.marketplace}\u0000${ext.extension}`),
      ),
    [installedExts],
  )
  const sidebarPanels = useMemo(
    () =>
      extPanels
        .filter((panel) => panel.def.location === 'sidebar')
        .map((panel) => ({
          item: {
            id: panel.view as ActivityView,
            path: panel.def.icon ?? '',
            label: panel.def.label,
          },
          hidden: railHidden.has(
            `${panel.extension.marketplace}\u0000${panel.extension.extension}`,
          ),
        })),
    [extPanels, railHidden],
  )
  const railExtras = useMemo(
    () => sidebarPanels.filter((p) => !p.hidden).map((p) => p.item),
    [sidebarPanels],
  )
  /*
   * The ones the user took off the strip. They still reach the rail — as menu entries under
   * `···` rather than as buttons — because a rail button is the only route into a contributed
   * panel, and a setting that removed the last route would be a one-way door with the way back
   * on a page the user has no reason to connect with the panel that vanished. See
   * `ActivityRail`'s `extraHidden`.
   */
  const railHiddenExtras = useMemo(
    () => sidebarPanels.filter((p) => p.hidden).map((p) => p.item),
    [sidebarPanels],
  )

  /*
   * The other half of the extension host: what a worker can learn about an open file, and what it
   * can put back.
   *
   * `jumpTo` before `file.open`, exactly as the Problems panel's `onOpenLocation` does and for its
   * reason: the editor for a path a worker names usually does not exist yet, so the reveal is
   * parked and spent by the mount the open causes. Going the other way round loses it.
   */
  useEffect(
    () =>
      attachEditorBridge((path, line, column) => {
        if (!activeProjectId) return
        jumpTo(activeProjectId, { path, line, column })
        // No hydrate afterwards: `tab_open_file` broadcasts the snapshot that makes the
        // tab appear, exactly as `gitDiff.openTab` documents.
        void fileApi.open(activeProjectId, path)
      }),
    [activeProjectId],
  )
  useEffect(() => {
    setBridgeProject(activeProjectId ?? null)
  }, [activeProjectId])

  /**
   * The tab and pane the user is looking at.
   *
   * Derived once and shared by the key context, the status bar and the command dispatcher.
   * Three separate derivations of "what is focused" is three chances for a command to act
   * on something other than what the `when` clause was evaluated against.
   */
  const focused = (() => {
    const tab = activeProject?.tabs.find((t) => t.id === activeProject.activeTab)
    if (!tab) return undefined
    const pane = tab.tree.panes[tab.tree.focused]
    if (!pane) return undefined
    return { tab, pane }
  })()
  const focusedSession = focused?.pane.session ?? undefined

  /**
   * The Claude pane a mention is *aimed* at.
   *
   * The focused pane when it is a Claude one; otherwise the project's console — `tabs[0]`'s
   * first Claude pane in map order. Falling back to the console rather than to nothing
   * matters because the common gesture is Ctrl+P from an editor, where no Claude pane is
   * focused by definition, and a mention that silently went nowhere would look identical to
   * one that worked.
   *
   * The comment here used to call that fallback "the console's *primary session*". It is not,
   * and the difference is load-bearing enough to be worth the correction: this takes the first
   * Claude pane the map yields, which need not be the primary one — and `Project.primary_session`
   * is in any case a field written once at project creation that commonly names a session no
   * pane holds. Neither this rule nor that field asks whether the pane it picks has a `claude`
   * running, which is why the pane below is a *preference*: `claude_send_lines` reroutes to a
   * Claude that can actually receive and answers with the pane it used.
   */
  const mentionTarget = (() => {
    if (focused?.pane.kind === 'claude') return focused.pane.id
    const console = activeProject?.tabs[0]
    if (!console) return undefined
    return Object.values(console.tree.panes).find((p) => p.kind === 'claude')?.id
  })()

  /*
   * Key context: the `when` clauses in the keymap are evaluated against this.
   *
   * Recomputed every render rather than memoised. It is six booleans, and a stale context is
   * how a binding fires in a state its `when` clause excludes — worse than the arithmetic it
   * would save.
   */
  const keyContext = {
    projectOpen: activeProjectId !== null,
    overlayOpen: overlay !== null,
    // Narrower than `overlayOpen`, and that is the whole reason it exists: ⌥L is scoped to the
    // one overlay that has a library scope, so it stays unbound — and therefore passes through —
    // in the palette, the symbol picker, the branch popup and every terminal pane in every
    // window. The gate is a *capture* listener, so an `overlayOpen` clause would have taken
    // readline's `downcase-word` from a shell any time a modal happened to be up.
    filePickerOpen: overlay === 'files',
    // `gate.ts` listens on window capture, so it sees a keystroke before an open menu does.
    // Nothing collides today — every default binding is a modified chord — but this is what
    // stops a user who rebinds a bare key from having it swallowed while a menu is up.
    contextMenuOpen,
    editorFocused: focused?.pane.kind === 'editor',
    terminalFocused: overlay === null && focused?.pane.kind !== 'editor',
    // Gates `claude.fork`, `claude.mirror` and `claude.split.newSession` — all three act on
    // the focused session, so none of them means anything without a Claude pane to act on.
    claudePaneFocused: focused?.pane.kind === 'claude',
    sidebarFiles: sidebar.view === 'files',
    sidebarGit: sidebar.view === 'git',
    // A diff surface is mounted and in front — a git diff tab, the log tool window's revision
    // diff, or the conflict resolver. Read from the claim stack rather than from the tree,
    // because that is where the answer actually is; `panes/changeNav.ts` has the argument, and
    // `cide_core::commands::CONTEXT_FLAGS` has why no derived spelling of it works.
    diffFocused,
  }

  /*
   * The one dispatcher. The key gate and the command palette both call it, which is what
   * stops a binding and its palette entry drifting into two different behaviours.
   *
   * Built fresh every commit — a stale `focused`/`activeProject` in a dispatcher is a real
   * bug the closures below guard against — but *published* through a ref behind a stable
   * wrapper. The identity is what every consumer keys on: `WorkspaceContent`'s memo, the
   * pinned `onSelectOpened` callback, the key gate's own `live` ref. `useInsertionEffect`
   * so the ref is reassigned before any layout effect or event handler of this commit can
   * run; `runCommand` is only ever called from handlers and the gate, never during render,
   * so it always dispatches against the tree the user is looking at.
   */
  const dispatcherRef = useRef<(command: string, args: unknown) => void>(() => {})
  const runCommand = useCallback(
    (command: string, args: unknown) => dispatcherRef.current(command, args),
    [],
  )
  const dispatcher = createDispatcher({
    fallback: (command) => {

      /*
       * The split family, including the two the project exists for.
       *
       * `claude.fork` and `claude.mirror` were in the command registry — so the palette
       * listed them — and in `SplitIntent`, and wired end to end through `spawnPlans` to a
       * `--fork-session` invocation verified against the real CLI. And nothing dispatched
       * them: running either from the palette fell through to a diagnostic log. The hardest
       * feature in the project was unreachable by any gesture, which is why "it is
       * implemented" and "a user can do it" are different claims.
       */
      if (!activeProject || !focused) {
        return diag.log(`no focused pane for: ${command}`)
      }
      const split = (axis: 'row' | 'col', intent: SplitIntent | null) =>
        void splitPane(activeProject.id, focused.tab.id, focused.pane.id, axis, 'after', intent)

      switch (command) {
        case 'pane.split.right':
          return split('row', null)
        case 'pane.split.down':
          return split('col', null)
        case 'claude.split.newSession':
          return split('col', { kind: 'newClaude' })
        case 'claude.fork':
          // Branches the focused conversation: shared history to this point, then divergent.
          return split('col', { kind: 'forkPrimary' })
        case 'claude.mirror': {
          // No new process — a second sink on the session already running. Meaningless
          // without one, so it reports rather than splitting into an empty pane.
          const session = focused.pane.session
          if (!session) return diag.log('mirror: the focused pane has no session yet')
          return split('row', { kind: 'mirror', session })
        }
        case 'terminal.splitBelow':
          return split('col', { kind: 'shell' })
        default:
          return diag.log(`command not handled by this window: ${command}`)
      }
    },
    /*
     * No `showSidebar` here any more. Revealing a panel is `chrome/panelRequests.ts`'s job,
     * registered by the effect near `sidebar`'s declaration above — because the dispatcher was
     * never the only caller that needed it, and a closure threaded through props could only ever
     * serve the callers that could reach the props. See that module and `DispatchDeps`.
     *
     * F4. The function is handed to `setSidebar` unwrapped because it *is* a state updater —
     * `(prev) => next`, closing over nothing — which is what keeps the decision about which
     * panel comes back in a module a check script can run rather than in this file.
     */
    toggleSidebar: () => setSidebar(toggleSidebar),
    /*
     * Which tab `file.save` writes. The buffer lives in a CodeMirror state inside the pane,
     * reachable only through the saver `editor/openBuffers.ts` holds for its tab — and the
     * dispatcher has no way to learn a tab id on its own, because the key gate runs outside
     * React and the palette is a list of strings.
     */
    focusedTab: () => focused?.tab.id ?? null,
  })
  useInsertionEffect(() => {
    dispatcherRef.current = dispatcher
  })

  /*
   * Stable handlers for the sidebar panels. Each panel host is `memo`ised (its definition
   * site says why), and a memo over a prop rebuilt every render is a memo that never holds
   * — so everything a panel receives from here is pinned with `useCallback` over at most
   * `activeProjectId` (a string) and the already-stable `runCommand`. The comments about
   * *what* each handler routes, and why through the command, stay at the JSX sites below,
   * where a reader meets them.
   */
  const onShowHistory = useCallback(
    (path: string) => runCommand('git.history.file', { path }),
    [runCommand],
  )
  const onSelectOpened = useCallback(() => runCommand('file.reveal', null), [runCommand])
  const onOpenPin = useCallback(
    (id: string) => {
      if (id === PROJECT_NOTES) runCommand('file.projectNotes', null)
    },
    [runCommand],
  )
  const onOpenTreeFile = useCallback(
    (path: string) => {
      if (!activeProjectId) return
      // `UNKNOWN_LINE`: the Explorer names a file and not a place in it, so the per-file
      // view memory decides where it opens. See `editor/navHistory.ts`.
      jumpTo(activeProjectId, { path, line: UNKNOWN_LINE, column: 1 })
      void fileApi.open(activeProjectId, path)
    },
    [activeProjectId],
  )
  const onGitUpdate = useCallback(() => runCommand('git.pull', null), [runCommand])
  const onOpenHit = useCallback(
    (path: string, line: number, column: number, endColumn: number) => {
      if (!activeProjectId) return
      jumpTo(activeProjectId, { path, line, column, endColumn })
      void fileApi.open(activeProjectId, path)
    },
    [activeProjectId],
  )
  const onOpenLocation = useCallback(
    (path: string, line: number, column: number) => {
      if (!activeProjectId) return
      jumpTo(activeProjectId, { path, line, column })
      void fileApi.open(activeProjectId, path)
    },
    [activeProjectId],
  )
  const onRefreshProblems = useCallback(() => {
    if (activeProjectId !== null) refreshDiagnostics(activeProjectId)
  }, [activeProjectId])
  const onRestartSource = useCallback(
    (id: string, label: string) => {
      if (activeProjectId !== null && isDiagnosticSourceId(id)) {
        restartDiagnosticSource(activeProjectId, id, label)
      }
    },
    [activeProjectId],
  )
  /*
   * Entry point 2 of the key gate: the window listener.
   *
   * Entry point 1 is `terminalKeyGate`, attached to every terminal in `paneHosts`. Both are
   * required and neither is sufficient: a window listener cannot stop `^P` reaching a PTY,
   * because xterm has already written the byte by the time the event bubbles.
   */
  useKeyGate({
    bindings: boot?.keymap ?? [],
    context: keyContext,
    run: runCommand,
  })

  // The status bar reports the session the user is looking at, not "a" session. With two
  // Claude panes side by side, showing whichever reported last would make the token figure
  // flicker between two conversations and belong to neither.
  //
  // The selector formats *inside* the store read and returns the string, never the
  // `bySession` map itself. The map's identity changes on every statusline frame — about
  // once a second per running session, for as long as it lives — and `App` has no memo
  // boundary above the pane grid, so selecting the map here re-rendered the entire window
  // once a second *per background session*, forever. A primitive compares by value, so this
  // component now re-renders only when the focused session's readout text actually moves.
  const claudeReadout = useSessionStatus((s) =>
    focusedSession ? formatClaude(s.bySession[focusedSession]) : undefined,
  )

  /**
   * Ctrl+click on a file path in a terminal pane's output.
   *
   * Curried by project because the two windows this component renders show different ones: the
   * shell window's active project, and the one owning a detached pane. Everything else is
   * identical, and one handler is the point — two copies is how the detached window ends up with
   * the reveal and not the open, or the other way round.
   *
   * The order is the same as the search panel's click and is a design rather than a preference:
   * the editor for this path usually does not exist yet, so the reveal is *parked* and spent by
   * the mount `openFromTerminal` causes. Requesting it afterwards would deliver it to nobody and
   * the file would open at line 1 — which, for a click on `src/main.rs:270:13`, has not gone
   * where the user pointed. `endColumn` is `column + 1`: this is a caret, not a range, and the
   * producer never said how long the thing at that column is.
   *
   * `openFromTerminal` and not `open`, and that is the whole security story in one word — see
   * `cmd::file::terminal_open_path`. A refusal is a sentence in the notice stack, because a
   * ctrl+click that silently does nothing is indistinguishable from a link wired to nothing.
   *
   * One refusal is a *question* instead of a sentence: a path outside every root. Whether it may
   * be asked is `terminal/outsideOpen.ts`'s rule and emphatically not this callback's — a rule in
   * a `.catch` is in the one place no check script can compile, which is where two shipped bugs
   * have already hidden. All this does is route: ask the module, park the question if there is
   * one, report the message if there is not.
   */
  const openTerminalPath = useCallback(
    (project: ProjectId) =>
      (path: string, at: { line: number; column: number } | null): void => {
        /*
         * KNOWN GAP, and it is written here rather than left to be rediscovered: this lands
         * the caret only when the tab opens in *this* window.
         *
         * `revealRequest` is a module-level map, and a detached pane (`pane:<uuid>`) is a
         * separate webview with its own module instances. `terminal_open_path` is a workspace
         * mutation, so the tab and its editor mount in the shell window — which never sees
         * the request parked here, and the caret sits at line 1 while the file itself opens
         * correctly.
         *
         * Not fixed in passing, deliberately. Carrying a caret across windows needs the
         * target to be durable state Rust owns, and no such field exists — there is no cursor
         * in `EditorViewState` for this to ride on. Inventing one at the end of a batch is
         * precisely how the previous batch nearly shipped a commit-corrupting bug, so it is
         * named as a gap instead of being half-built.
         */
        // Re-parked on the retry rather than only once, because `REVEAL_TTL_MS` is 10 seconds
        // and reading a confirmation can easily take longer than that. Without this the approved
        // open lands at line 1 — the file opens, and the caret quietly does not go where the
        // user pointed, which is the shape of bug that gets reported months later as "sometimes".
        const park = (): void => {
          if (at === null) return
          jumpTo(
            project,
            { path, line: at.line, column: at.column, endColumn: at.column + 1 },
            // Never from here. See `pendingJump` below.
            false,
          )
        }
        /*
         * The history entry is prepared before the call and **written only once it succeeds**,
         * and that is a fix rather than a tidy-up.
         *
         * It used to be written by `park`, on the first attempt, before anything was asked. So a
         * path the user was shown and **declined** — the out-of-project confirmation — stayed on
         * the Back stack, and Back then walked into it through `tab_open_file`, which by its own
         * documentation "enforces nothing": no containment, no `is_file`, none of the four
         * guards `openable` applies to exactly this class of path. A refusal the user had just
         * given was reachable again with a thumb button, and the same door reopened a declined
         * FIFO, which parks a blocking-pool worker in `read_to_end` for ever.
         *
         * `navHistory.ts` already states the rule this restores: an entry is a place the user
         * has *been*. A refused open is not one. The reveal still has to be parked first — it
         * has a TTL and the open is what mounts the editor that spends it — which is why the two
         * halves are separable at all, and why `pendingJump` exists beside `jumpTo`.
         *
         * Two halves and not one call in the `.then()`, because the *origin* must be read here,
         * synchronously, while the caret is still in the editor the user is leaving. See
         * `pendingJump`: the open broadcasts from inside the workspace lock, so the destination's
         * editor can have mounted and claimed the caret slot before this promise settles.
         *
         * Recorded once and not per retry: an attempt that throws never reaches the commit, so
         * the approved retry is the first and only writer. That also preserves what the old
         * first-attempt-only flag was for — two identical origin/destination pairs on the stack
         * would make Back appear to do nothing, arriving at an entry indistinguishable from the
         * one it left.
         */
        const attempt = (approvedTarget?: string): void => {
          park()
          const arrived =
            at === null ? null : pendingJump(project, { path, line: at.line, column: at.column })
          void fileApi
            .openFromTerminal(project, path, approvedTarget)
            .then(() => {
              // After the open, so a refusal never becomes a place the user has been. The
              // snapshot that makes the tab renderable rides the open's own broadcast — no
              // re-read here — and the comment above already concedes that broadcast can
              // beat this promise, which is why `arrived` is a parked commit rather than an
              // ordering guarantee.
              arrived?.()
            })
            .catch((reason: unknown) => {
              const ask = outsideAsk(reason)
              if (ask === null) return notifyFailure(reason)
              requestOutsideOpen({ ask, proceed: () => attempt(ask.target) })
            })
        }
        attempt()
      },
    [],
  )

  // A `pane:<uuid>` window shows exactly one pane. It shares the workspace mirror with the
  // shell window but none of its chrome: no project tabs, no rail, no status bar.
  if (boot?.role.kind === 'detachedPane') {
    const { project, tab: homeTab, pane } = boot.role
    const owner = boot.workspace.projects[project]
    const detachedPane = owner?.detached[pane]
    // Nothing until the project's plan is in, for the reason on `restorePlan` above. This
    // window is the one that suffered most from doing otherwise: it never received a plan at
    // all, so a torn-out pane came back at launch, adopted a `SessionId` from the process that
    // had written `workspace.json`, and printed `— no such session —` into a blank pane every
    // time.
    if (!plannedProjects.has(project)) return <WindowFrame>{null}</WindowFrame>
    if (!owner || !detachedPane) {
      // The domain no longer holds this pane — its project closed while the window was up.
      // Rendering nothing is honest; the window closes on the next snapshot.
      return <WindowFrame>{null}</WindowFrame>
    }
    return (
      <>
        <DetachedPaneWindow
          pane={detachedPane}
          // A pane opened onto an agent run's conversation spawns in that run's directory —
          // its worktree — which is where the harness filed the transcript (M42).
          cwd={detachedPane.continues?.cwd ?? owner.roots[0]?.path ?? '.'}
          project={project}
          roots={owner.roots.map((r) => r.path)}
          // The plan entry for this pane. `lifecycle::plan_restore` has always walked
          // `project.detached` and produced one; this branch simply never read it, so the one
          // thing it knows — that `pane.session` names a conversation from a previous process,
          // and whether it can be resumed — reached nobody.
          restore={restorePlan.get(pane)}
          // A torn-out pane opens tabs in the shell window, which is correct and needs no
          // special case: `terminal_open_path` is a workspace mutation, so it broadcasts
          // `cide://workspace-changed` and the shell picks the tab up like any other.
          onOpenPath={openTerminalPath(project)}
          // The same write the shell branch makes, and it has to be made from here too: this
          // window is where a long turn is watched and therefore where a restart happens, and
          // `bind_session` reaches `project.detached` precisely so this call has somewhere to
          // land. Without it the workspace keeps naming the dead child.
          onSessionBound={(session) => void bindSession(project, homeTab, pane, session)}
          onRedock={() => void redockPane(boot.window)}
        />
        {/*
          * A detached window returns before the shell below, so it had no `Failures` at all —
          * the one window kind where every gesture is new this round, and the only one where
          * a rejected command had nowhere to be seen. Same reasoning as the shell's copy.
          */}
        <Failures />
        {/*
          * And the same reasoning again, one refusal further on. A torn-out pane's output names
          * out-of-project paths exactly as a docked one does, and this branch returns before the
          * shell's copy below — so a gate wired only into the shell tree would leave a ctrl+click
          * here asking nobody and opening nothing. That is the shape of defect this batch's other
          * half was fixing in `restorePlan`; it does not get to reappear in the same file.
          */}
        <OutsideOpenGate />
        {/* The whole event behind a rendered JSON log line, opened by clicking its timestamp.
            In both branches for the reason above it: a detached pane draws log lines too. */}
        <LogDetailCard />
        <PullStrategyGate />
        <PushDialog />
        <ConflictsDialog />
        {/*
          * The properties card, and `onOpenHistory` is deliberately **not** passed here.
          *
          * `file.properties` carries no `shellWindow` clause — an overlay is raisable from any
          * window, which is `git.blame`'s stated precedent — so the command is live in this
          * window and the card has to be mounted in this branch or the keystroke asks nobody at
          * all, with nothing on screen and nothing logged. But a detached-pane window has no
          * tool window, so the one control that would open one is simply not drawn, rather than
          * being offered and then reporting that this window cannot do it.
          */}
        <FileProperties />
      </>
    )
  }

  return (
    <WindowFrame>
      <div className={styles.app}>
        {/*
          * A `tab:<uuid>` window renders THIS same shell path — same `TabContent`, same
          * `PaneFrame`/`PaneBody`, same status bar and dialogs — because a torn-out tab is
          * not a different kind of tab; `DetachedPaneWindow`'s header states what a second
          * rendering costs. What it does not get is the chrome that belongs to the shell:
          * the project strip (this header), the activity rail, the sidebar and the tab
          * strip, each gated on `tabWindow` below. Its own header carries the window's
          * lights, the drag region and the way back.
          */}
        {tabWindow ? (
          <DetachedTabHeader tab={windowTab} onRedock={() => void redockTab(windowLabel())} />
        ) : (
        <AppHeader
          projects={projects}
          activeProject={activeProjectId}
          /*
           * Clicking a project tab did nothing until now. `activate_project` has been in the
           * domain since projects went multi-window and no command exposed it, so with two
           * projects open there was no way to switch between them — the same dead control
           * the sidebar's search button was. Through the store's mutator, which is where the
           * wait-for-the-snapshot lives.
           */
          onActivate={(id) => void activateProject(id as ProjectId)}
          onClose={(id) => void closeProject(id)}
          onToggleTheme={togglePersistedTheme}
          /*
           * The header's ⊞ and ⧉ aim at the focused pane. Omitting them renders the buttons
           * disabled rather than inert, which is the contract `AppHeaderProps` documents —
           * so with no project open they are visibly unavailable instead of silently doing
           * nothing.
           */
          {...(activeProject && focused
            ? {
                onSplit: () =>
                  void splitPane(activeProject.id, focused.tab.id, focused.pane.id, 'row', 'after'),
                onDetach: () =>
                  void detachPane(activeProject.id, focused.tab.id, focused.pane.id),
              }
            : {})}
        />
        )}

        <div className={styles.body}>
          {/*
            * The badge is withheld at zero — a clean tree needs no ornament — so the audit has
            * to ask for a count or it measures a state the chrome never shows it. That flag
            * used to be the *only* thing feeding this prop: `gitDirty={auditMode()}` wired the
            * badge to a query parameter and to nothing else, so outside `?audit=1` it never
            * appeared in any repository state. The count is now real and the fixture is the
            * override, which is the way round every other audit-driven surface here works.
            */}
          {/* No rail in a torn-out tab window: every button on it opens a panel this window
              does not render, and the sidebar state was initialised closed to match. */}
          {!tabWindow && (
          <ActivityRail
            active={sidebar.view}
            changed={auditMode() ? AUDIT_GIT_CHANGES : gitChanged}
            errors={diagCounts?.errors ?? null}
            /* "Something is looking right now" — the panel-model rule (`anyScanning`), not a
               local re-derivation: the first pass and a later re-index report it through
               different arms of the snapshot, and this is the one place that must not
               disagree with the panel about whether work is happening. */
            busy={anyScanning(diagnostics.snapshot)}
            /* `openCount` answers `null` for every board arm but `ready` — including `absent`,
               because a project with no tracker has no count, it has no tracker. The rail draws
               nothing for `null` and nothing for `0`, and those are two different claims: nobody
               looked, against looked and the tracker is clear. */
            tasks={openCount(taskBoard)}
            /* `liveCount` answers `null` for every roster arm but `ready` — nobody has looked,
               subagents are off, or no roles are defined — and a *number* only when something
               counted. The rail draws nothing for `null` and nothing for `0`, and those are two
               different claims: nobody looked, against looked and every role is idle. With no
               registry behind this slice a `ready` roster has no runs, so the honest badge is
               absent, which is exactly what `0` produces. */
            agents={liveCount(agentRoster)}
            /* The one agent state that is a call to action, drawn in the error tone. It cannot
               make a badge appear on its own — with `agents` `null` or `0` there is no pill to
               recolour — which is why it is a separate flag rather than a fourth count. */
            agentsAwaiting={
              agentRoster.kind === 'ready' &&
              agentRoster.runs.some((run) => run.phase === 'awaitingPermission')
            }
            /*
             * The buttons extensions contribute, in registry order. (M22)
             *
             * Only the ones a *sidebar* panel was declared for — a `bottom` panel is a tab in the
             * tool window and has no rail button, and drawing one for it would light a rail
             * button that opens nothing.
             *
             * `icon` is a 24×24 SVG path `d` validated by `cide_ext::manifest::is_svg_path` at
             * install, which is a whitelist rather than an escape: this value goes into an
             * attribute the rail renders, and that is the one place a manifest could otherwise
             * put markup. A panel with no icon gets an empty path, which draws a blank button —
             * and the manifest reader has already warned its author about exactly that.
             */
            extra={railExtras}
            extraHidden={railHiddenExtras}
            onSelect={(next) => {
              // What a click means — the lit one toggles shut, any other switches, ⚙ has no
              // panel to hide — is `selectView`'s, in `chrome/sidebarView.ts`, along with the
              // memory of which panel F4 restores. What is left here is the side effect only
              // this component can perform: Settings is a workspace *tab* rather than a
              // sidebar view, so choosing it also opens (or re-activates) the project's tab.
              setSidebar((s) => selectView(s, next))
              if (next === 'settings') {
                if (activeProjectId) {
                  // The tab appears on the mutation's own broadcast — no re-read.
                  void settingsApi.openTab(activeProjectId)
                } else {
                  /*
                   * No project, so no tab to put it in — the frame instead. (M74)
                   *
                   * This branch used to be the `&& activeProjectId` half of the condition
                   * above, which made the button take a click and do nothing whenever the
                   * window was empty. Set rather than toggled, so ⚙ behaves the same way in
                   * both branches: with a project it opens-or-activates a tab, and here it
                   * opens-or-keeps the frame. The `×` is the way out of either.
                   */
                  setSettingsFrame(lastFrameSection())
                }
              }
            }}
            /*
             * The projectless shell's panel is `Workspace.toolWindow` — a real second home for
             * this state, not a fallback. `?? false` for the frame before bootstrap resolves.
             */
            toolWindowOpen={
              (activeProject?.toolWindow ?? boot?.workspace.toolWindow)?.open ?? false
            }
            onToggleToolWindow={() => {
              // Straight to Rust, with no local mirror: the panel's state lives on
              // `Project::tool_window`, so the answer comes back as a `cide://workspace-changed`
              // snapshot like every other workspace mutation. A `useState` here would be a
              // second copy of one boolean, which is the shape `chrome/sidebarView.ts` records
              // going wrong.
              //
              // `null` for the empty frame rather than a refusal: this used to be
              // `if (!activeProject) return`, which is why the button was inert with nothing
              // open. Which panel a `null` names is Rust's answer and only Rust's — see
              // `cide_core::toolwindow`.
              const state = activeProject?.toolWindow ?? boot?.workspace.toolWindow
              if (state === undefined) return
              void toolWindowApi
                .setLayout(activeProject?.id ?? null, { open: !state.open })
                .catch(() => {})
            }}
          />
          )}
          {/*
           * Everything but the rail, stacked.
           *
           * The activity rail stays a child of `.body` so it runs the **full height**; this
           * column holds the rest, and the git tool window is its last child. That is the
           * difference between "full width" and "full width except the icon bar", and it is the
           * second one that was asked for: the rail is a permanent strip down the side of the
           * window, not something a panel at the bottom should cut off.
           *
           * The sidebar *panel* is inside `.upper`, so the tool window runs underneath it too.
           * That is deliberate and is the whole point of the request — the panel's left edge is
           * a fixed 42px and does not move when Files or Git is opened or closed. Putting the
           * tool window inside `.upper` instead would make its width depend on the sidebar, so
           * opening a panel would narrow it and closing one would widen it, which reads as the
           * bottom of the window shifting for no reason.
           */}
          <div className={styles.workArea}>
            <div className={styles.upper}>

            {/*
              * The sidebar. Four of the rail's five views have a panel; `settings` opens a
              * workspace tab instead, which is why it is absent here rather than empty.
              *
              * `search` and `problems` used to render nothing, on the reasoning that an empty
              * panel implies a missing feature. That was wrong and a user reported it: the
              * rail still draws the button and still highlights it on click, so rendering
              * nothing does not read as "not built" — it reads as broken. A dead control is
              * worse than either a real panel or an absent one.
              */}
            {sidebar.view === 'files' && (
              <PanelBoundary name="Files" onClose={() => setSidebar((st) => selectView(st, 'files'))}>
                <Explorer
                  project={activeProjectId}
                  /*
                   * *Select opened file*, and the two halves are deliberately different in kind.
                   *
                   * `openedFile` is a *fact* — the path the active tab is about — and it only
                   * decides whether the button is enabled or greyed with a reason. `onSelectOpened`
                   * runs the **command**, which re-derives that same path from the same function.
                   * That looks redundant and is not: the button, ⌃⇧E and the palette row all end in
                   * one handler with one copy of the rules, and a button that called
                   * `treeStore.reveal` itself would be a second call site to keep in step.
                   */
                  openedFile={focusedTabPath(boot)}
                  onSelectOpened={onSelectOpened}
                  /*
                   * A pinned row's open, routed through the **command** for the same reason
                   * `onSelectOpened` above is: the double-click, the palette row and any chord a
                   * user binds in `keymap.json` must not become three code paths that behave in
                   * three ways. The id is checked rather than assumed — a pin this build has never
                   * heard of (an older webview against a newer backend) does nothing rather than
                   * running a command that does not exist.
                   */
                  onOpenPin={onOpenPin}
                  /*
                   * *Show File History* on a tree row, through `runCommand` for the reason the two
                   * handlers above route that way: the preconditions — is this a shell window, does
                   * the project hold a repository, which repository is this path in — live in one
                   * dispatch case, and the tab strip's copy of this item already goes through it.
                   */
                  onShowHistory={onShowHistory}
                  onOpenFile={onOpenTreeFile}
                />
              </PanelBoundary>
            )}
            {sidebar.view === 'git' && (
              /*
               * Every sidebar panel is wrapped, and the Git one is why. A throw in here used to
               * unmount the whole root — no chrome, no tabs, no terminals — and since the rail's
               * choice is restored on launch, the window came back empty on every restart with no
               * message and no reachable way out. `PanelBoundary` keeps the failure inside the
               * panel and puts the close button in the fallback, so recovery is reachable from
               * inside the failure rather than from the chrome the failure just destroyed.
               */
              <PanelBoundary name="Git" onClose={() => setSidebar((st) => selectView(st, 'git'))}>
                <GitPanel
                  project={activeProjectId}
                  // The same routing again, from the changes tree's file rows. The panel joins its
                  // repo-relative path to the repository's root before calling this, because every
                  // other surface hands over an absolute one and `git.history.file` expects it.
                  onShowHistory={onShowHistory}
                  // And the toolbar's *Update project*, which had no command to call until M20.
                  // Through `runCommand` rather than `branchApi.pull`, so the divergence dialog,
                  // the retry and the aggregated notice are the key's code path and not a second
                  // copy of it.
                  onUpdate={onGitUpdate}
                />
              </PanelBoundary>
            )}
            {sidebar.view === 'search' && (
              <PanelBoundary name="Search" onClose={() => setSidebar((st) => selectView(st, 'search'))}>
                <SearchPanel
                  project={activeProjectId}
                  // The caret, which is the half the user reported missing — a search result
                  // that opens the file at the top has not gone to the found place. `jumpTo`
                  // also records the visit, so the mouse's Back button returns here — and it is
                  // requested BEFORE the open, which is the design rather than a preference:
                  // the editor for this path usually does not exist yet, so the request is
                  // parked and spent by the mount the `file.open` causes. A file already open
                  // is revealed immediately instead. See `editor/revealRequest.ts` — the parked
                  // request expires, so one for a file that never opens cannot fire when the
                  // user opens it by hand an hour later.
                  onOpenHit={onOpenHit}
                />
              </PanelBoundary>
            )}
            {sidebar.view === 'problems' && (
              <PanelBoundary name="Problems" onClose={() => setSidebar((st) => selectView(st, 'problems'))}>
                <ProblemsPanel
                  project={activeProjectId}
                  snapshot={diagnostics.snapshot}
                  hidden={diagnostics.hidden}
                  // The two controls the M18 report asked for. Both go through
                  // `sidebar/ProblemsPanel/actions.ts`, which `keys/dispatch.ts` also calls for
                  // `problems.refresh` — one implementation, so the button and the palette row cannot
                  // drift into doing different things.
                  onRefresh={activeProjectId === null ? undefined : onRefreshProblems}
                  // `onRestartSource` checks rather than casts its id: `SourceRow.id` is a
                  // plain string because `model.ts` imports nothing, so the pinned handler is
                  // where the two meet. The set is open since M22 — an extension may
                  // contribute a server — so what is asserted is the one property the id must
                  // have, and `sourceRows` decides which rows offer the button at all.
                  onRestartSource={activeProjectId === null ? undefined : onRestartSource}
                  // Requested BEFORE the open — the editor for this path usually does not
                  // exist yet, so the request is parked and spent by the mount the open
                  // causes. The same ordering the search panel's `onOpenHit` uses; see
                  // `editor/revealRequest.ts`.
                  onOpenLocation={onOpenLocation}
                />
              </PanelBoundary>
            )}
            {/*
              * Tasks. (M18)
              *
              * Everything it draws is a prop, and everything it writes goes through
              * `sidebar/tasksStore.ts`; the attach and the `cide://tasks-changed` subscription are
              * above, in this component, because the rail's badge outlives the panel.
              *
              * `project` is the only prop, and `null` is a state it draws rather than a reason to
              * render nothing: a detached-pane window has no project, and the panel says so.
              */}
            {sidebar.view === 'tasks' && (
              <PanelBoundary name="Tasks" onClose={() => setSidebar((st) => selectView(st, 'tasks'))}>
                <TasksPanel project={activeProjectId} />
              </PanelBoundary>
            )}
            {/*
              * Agents. (M18)
              *
              * Everything it draws is a prop, and everything it writes goes through
              * `sidebar/agentsStore.ts`; the attach and the `cide://agents-changed` subscription
              * are above, in this component, because the rail's badge outlives the panel.
              *
              * `project` is a state it draws rather than a reason to render nothing: a
              * detached-pane window has no project, and the panel says so instead of appearing
              * blank — which is what this branch's absence used to produce for the whole view.
              *
              * The agent→task link needs nothing from here any more: the panel selects the
              * task in `tasksStore`, and the card is `TaskDetailHost`'s — mounted below,
              * outside these branches — so it opens over this panel instead of switching the
              * sidebar to Tasks.
              */}
            {sidebar.view === 'agents' && (
              <PanelBoundary name="Agents" onClose={() => setSidebar((st) => selectView(st, 'agents'))}>
                <AgentsPanel project={activeProjectId} />
              </PanelBoundary>
            )}
            {/*
              * OpenSpec. (M28)
              *
              * Below Agents and above Extensions, matching the rail's own order. Like every panel
              * here it draws props only; the `cide://spec-changed` subscription and the attach
              * are above, in this component, for the reason the Agents comment gives — the rail's
              * badge outlives the panel.
              */}
            {sidebar.view === 'openspec' && (
              <PanelBoundary
                name="OpenSpec"
                onClose={() => setSidebar((st) => selectView(st, 'openspec'))}
              >
                <OpenSpecPanel project={activeProjectId} />
              </PanelBoundary>
            )}
            {sidebar.view === 'extensions' && (
              <PanelBoundary
                name="Extensions"
                onClose={() => setSidebar((st) => selectView(st, 'extensions'))}
              >
                <ExtensionsPanel project={activeProjectId ?? null} />
              </PanelBoundary>
            )}
            {/*
              * Every panel an extension contributes, in one branch. (M22)
              *
              * A literal `{sidebar.view === …` block like the eight above it rather than a loop
              * over the contributed panels, and that is deliberate: `ui/scripts/check-boundary.mjs`
              * finds the rail's panels by scraping this file for exactly this shape and asserts
              * each one is wrapped. A `<PanelSlot>` loop would compile, render, and quietly take
              * that gate with it — an unwrapped panel takes the **whole window** down when it
              * throws, and the rail's choice is restored on launch, so it stays down.
              *
              * The `PanelBoundary` here is the outer of two guards and it should never fire: an
              * extension's code runs in a worker, so what this wraps is cide's own renderer over a
              * view model that has already been validated. It is here because "should never fire"
              * is not a thing to leave to a comment.
              */}
            {sidebar.view !== null && sidebar.view.startsWith('ext:') && (
              <PanelBoundary
                name="an extension panel"
                onClose={() => setSidebar((st) => selectView(st, 'files'))}
              >
                <ExtSidebarPanel view={sidebar.view} panels={extPanels} />
              </PanelBoundary>
            )}
            {/* The sidebar's drag edge — one handle for every panel, because there are only four
                widths: `--w-sidebar-files` sizes the explorer, search, problems and Extensions,
                `--w-sidebar-git` sizes git, `--w-sidebar-agents` sizes Agents and Tasks together,
                and `--w-sidebar-ext` sizes every panel an extension contributes. The Agents/Tasks
                pairing is `sidebarWidth.ts`'s decision and it argues it: the panel a user widens to
                read task titles in one is the panel they are about to read task titles in in the
                other, so a user who drags one and finds the other narrower has been given two
                settings for one intent. The contributed panels share a number for a different
                reason — `SidebarSettings::ext_width` — which is that a per-extension width cannot
                be a field of a fixed struct. Absent under `settings`, which has no panel, and while
                the sidebar is hidden: a handle with nothing to its left is a grab that resizes
                something the user cannot see. */}
            {isPanelOpen(sidebar) && (
              <SidebarSplitter
                panel={
                  sidebar.view === 'git'
                    ? 'git'
                    : sidebar.view === 'agents' ||
                        sidebar.view === 'tasks' ||
                        sidebar.view === 'openspec'
                      ? // The `agents` width token, reused rather than a fifth of its own:
                        // `check:sidebar` pins the token list, and a new one would mean editing
                        // `sidebarWidth.ts`, `tokens.css`, Rust's `SidebarSettings` and that
                        // check — for a panel that wants exactly the width these two already do.
                        'agents'
                      : sidebar.view?.startsWith('ext:') === true
                        ? 'ext'
                        : 'files'
                }
              />
            )}

            <div className={styles.content}>
              <WorkspaceContent
                activeProject={activeProject}
                settingsFrame={settingsFrame}
                onCloseSettingsFrame={closeSettingsFrame}
                visibleTabs={visibleTabs}
                tabRole={tabRole}
                tabWindow={tabWindow}
                restorePlan={restorePlan}
                plannedProjects={plannedProjects}
                runCommand={runCommand}
                openTerminalPath={openTerminalPath}
              />
            </div>
            </div>

            {/*
              * The git tool window. (M19)
              *
              * The last child of `.workArea` and a sibling of `.upper`, which is what decides its
              * width: everything except the activity rail. Two earlier arrangements were wrong in
              * opposite directions and both are worth naming, because each looks right on its own.
              *
              * Inside `.content` it spanned only the panes, so opening the Files panel narrowed
              * it — that is IDEA's *widescreen* variant, which is an option there and was not
              * asked for. A sibling of `.body` it spanned the entire window, cutting the activity
              * rail off above it — but the rail is a permanent strip down the side, not something
              * a panel at the bottom should truncate. Here it clears the rail and runs under the
              * sidebar panel, so its left edge is a fixed 42px that does not move when a panel is
              * opened or closed.
              *
              * The sidebar's own arithmetic is untouched either way: `chrome/sidebarWidth.ts`
              * measures a width against the window, and a panel that takes height off the bottom
              * changes no width at all.
              *
              * Three guards, each a bug if dropped. `auditMode` for the reason `TabContent` carries
              * it above: `CIDE_AUDIT=1` measures the chrome against the design mock, and the mock
              * has no tool window — withholding it is what keeps those 48 rows untouched.
              * `benchMode` matches its sibling: the IPC gate should not be racing a `git log` walk.
              * And `role.kind === 'shell'` closes a hole rather than tidying: a `DetachedTab` window
              * renders this same chrome and names a project the shell may also be showing, so
              * without it two windows would share one project's panel — the exact cross-window
              * bleed that keeping the state on `Project` exists to prevent.
              */}
            {/*
              * `project` of `null` is the empty frame's own panel (M74), whose state is
              * `Workspace.toolWindow`. The `activeProject !== null` guard that used to be in
              * this condition is what made the rail's ▤ button inert with nothing open. Its Log
              * tab draws a sentence rather than a commit list — there are no repositories to
              * walk — and its Docker tab is exactly as live as it is anywhere else, because a
              * daemon is a property of the machine and not of a checkout.
              */}
            {!benchMode() && !auditMode() && boot?.role.kind === 'shell'
              && (activeProject?.toolWindow ?? boot.workspace.toolWindow).open && (
              <PanelBoundary
                name="the git tool window"
                onClose={() => {
                  void toolWindowApi
                    .setLayout(activeProject?.id ?? null, { open: false })
                    .catch(() => {})
                }}
              >
                <ToolWindowHost project={activeProject?.id ?? null} />
              </PanelBoundary>
            )}
          </div>
        </div>

        {/*
          * Ctrl+P and Shift+Ctrl+P. Rendered last so the overlay stacks above the chrome
          * without a z-index fight, and only when one is actually open — an always-mounted
          * host would keep the picker's poll alive against a project nobody is looking at.
          */}
        {overlay !== null && activeProjectId && boot && (
          <OverlayHost
            project={activeProjectId}
            commands={boot.commands}
            keymap={buildKeymap(boot.keymap)}
            context={keyContext}
            actions={{
              /*
               * `jumpTo` **before** `fileApi.open`, and the order is the design rather than a
               * preference: the editor for this path usually does not exist yet, so the reveal
               * `jumpTo` issues is parked and spent by the mount the open causes. The same
               * sequence the search panel's `onOpenHit` uses — see `editor/revealRequest.ts`.
               *
               * `jumpTo` rather than `requestReveal` directly, here and at every other
               * navigation in this file: it is the single seam that also records the jump for
               * Back/Forward. `check-editor.mjs` asserts no `requestReveal(` survives outside
               * `editor/jump.ts` and `editor/goToDefinition.ts`, because "remember to record"
               * spread over eight call sites is a rule enforced by memory.
               *
               * For the File Structure popup the file is already open, so the reveal is
               * delivered live and the `open` is a no-op that re-activates the tab.
               */
              goToSymbol: (path, line, column, endColumn) => {
                closeOverlay()
                /*
                 * `focus: true`, and it fixes a defect that predates Go to line.
                 *
                 * `closeOverlay()` unmounts the card whose `<input>` held focus, so
                 * `activeElement` falls to `<body>`. The reveal receiver in `EditorSurface`
                 * deliberately does not focus — that rule was written for a *click* on a search
                 * result, where stealing focus would end the results list's ArrowDown/Enter walk
                 * — so Ctrl+F12 → ⏎ moved the caret and left the keyboard pointing at nothing:
                 * arrows and typing went nowhere until the user clicked into the buffer.
                 *
                 * `align: 'center'` for the same class of reason. A minimal scroll is right when
                 * the app moved the view on the user's behalf; this is a place they named, and a
                 * declaration landing flush against the bottom edge shows the signature with none
                 * of the body under it.
                 */
                jumpTo(activeProjectId, { path, line, column, endColumn, focus: true, align: 'center' })
                void fileApi.open(activeProjectId, path)
              },
              /*
               * Go to line. No `file.open`: the popup will not open without a caret, so the file
               * is by construction the focused editor's and its tab is already active — and the
               * reveal is therefore delivered live rather than parked.
               *
               * `endColumn: column` is an empty selection at the caret, which is what a line jump
               * means. `revealRange` clamps the line to the document, so a number past the end of
               * a file that shrank since the popup opened lands on the last line instead of
               * throwing out of `dispatch` and taking the React root with it.
               */
              goToLine: (path, line, column) => {
                closeOverlay()
                jumpTo(activeProjectId, {
                  path,
                  line,
                  column,
                  focus: true,
                  align: 'center',
                })
              },
              openFile: (path) => {
                closeOverlay()
                // No position: the picker names a file. See the Explorer above.
                jumpTo(activeProjectId, { path, line: UNKNOWN_LINE, column: 1 })
                void fileApi.open(activeProjectId, path)
              },
              openFileInSplit: (path) => {
                closeOverlay()
                jumpTo(activeProjectId, { path, line: UNKNOWN_LINE, column: 1 })
                void fileApi.open(activeProjectId, path)
              },
              mentionFile: (path) => {
                closeOverlay()
                if (!mentionTarget) {
                  // No Claude anywhere in this project to mention into. Saying so beats a
                  // silent no-op that looks exactly like success.
                  diag.log(`no Claude pane to mention ${path} into`)
                  return
                }
                // `claudeSend.lines` is the only route now: the `claude.mentionFile` this used
                // to have as an alternative ended in `.catch(() => {})`, so ⌥⏎ against a
                // project whose `claude` never completed the IDE handshake resolved happily
                // and did nothing — the silent no-op this whole round exists to end. It and
                // its Rust command are deleted. Empty text and no line numbers is a whole-file
                // mention. Deliberately uncaught: `Failures` turns the rejection into the
                // sentence saying why nothing was sent.
                //
                // …then take the user to the pane that got it. Without this the mention lands
                // in a prompt that may be in another tab, and Ctrl+P's ⌥⏎ looks exactly like
                // the silent no-op it used to be — the same complaint in a new disguise. The
                // reveal is deliberately after the send resolves: nothing should move if the
                // send failed, and `Failures` is already reporting that.
                //
                // `sent.pane`, **not** `mentionTarget`: Rust reroutes when the pane named here
                // has no `claude` on the IDE server, so revealing what we asked for would show
                // an empty prompt while the lines sat in another conversation. See
                // `cmd::file::claude_send_lines`.
                void claudeSend
                  .lines(activeProjectId, mentionTarget, path, '')
                  .then((sent) => revealPane(activeProjectId, sent.pane))
              },
              /*
               * A new scratch file, in the order that makes it feel like one gesture: create,
               * open, select.
               *
               * The reveal is **last and its answer is read**, which is the rule `file.reveal`
               * exists to enforce: `fs_scratch_new` re-lists the group before it resolves, so a
               * `false` here means something genuinely went wrong rather than "not yet", and
               * saying nothing about it would be the silent no-op this whole round is about.
               *
               * No `jumpTo`: a scratch is empty and has no place in it to go to, and recording a
               * navigation entry for a file with one line would put a Back stop on nothing.
               * `Failures` turns a rejected `scratchNew` into the sentence that says why —
               * deliberately uncaught, like every other command call in this file.
               */
              createScratch: (ext) => {
                closeOverlay()
                void fsApi.scratchNew(activeProjectId, ext).then(async (path) => {
                  // The tab rides the open's own broadcast; `useFileTree.reveal` below asks
                  // the tree store, not the mirror, so nothing here waits on a snapshot.
                  await fileApi.open(activeProjectId, path)
                  setSidebar((s) => showPanel(s, 'files'))
                  const shown = await useFileTree.getState().reveal(path)
                  if (!shown) {
                    notify('The scratch file was created but has no row in the tree yet.', {
                      kind: 'warn',
                      hint: 'Fold and unfold Scratches to re-read the drawer.',
                    })
                  }
                })
              },
              runCommand: (id) => {
                closeOverlay()
                runCommand(id, null)
              },
              runCommandInNewSession: (id) => {
                closeOverlay()
                runCommand(id, { newSession: true })
              },
            }}
          />
        )}

        {/*
          * Last in the shell — though that is no longer what makes it paint over everything,
          * and the distinction cost a bug: `OverlayCard` portals to `document.body`, so a
          * dialog is a later sibling than this stack whatever order it is written in here. The
          * `z-index: 90` in `Failures.module.css` is the part that is load-bearing now, and it
          * says why. A failed command used to be indistinguishable from a control wired to
          * nothing — which is precisely how the `+ row` buttons presented when the running
          * binary predated their command. See `Failures.tsx`; it needs no props because it
          * listens for the rejections the `void` call sites above it discard.
          */}
        <Failures />

        {/* The out-of-project confirmation. Also in the detached branch above — one gesture,
            two window kinds, and neither of them may be the one that asks nobody. */}
        <OutsideOpenGate />
        {/* The whole event behind a rendered JSON log line, opened by clicking its timestamp.
            In both branches for the reason above it: a detached pane draws log lines too. */}
        <LogDetailCard />
        <PullStrategyGate />
        <PushDialog />
        <ConflictsDialog />
        {/* The shell has a tool window, so here the card may offer the way into it. The repo and
            the repo-relative path come off the answer the card already holds, so this route does
            not re-run `git_locate` the way `git.history.file` has to. */}
        <FileProperties
          onOpenHistory={(repo, relPath) => {
            const project = activeProjectId
            if (project === null) return
            void toolWindowApi.openHistory(project, repo, relPath).catch(() => {})
          }}
        />

        {/*
          * The open task's card, over whichever panel selected it. Outside every `sidebar.view`
          * branch on purpose: the Agents panel's task links select a task and nothing else, so
          * the card must not need the Tasks panel to be the view — see `TaskDetailHost`'s
          * header. The host renders `null` until the selection resolves against a ready board,
          * and the boundary is the sidebar panels' rule applied to a portal: a throw in the card
          * must not empty the window, and Close clears the selection, which unmounts this
          * branch and thereby resets the boundary for the next open.
          */}
        {taskSelected !== null && (
          <PanelBoundary name="Task" onClose={() => useTasks.getState().select(null)}>
            <TaskDetailHost />
          </PanelBoundary>
        )}

        {/*
          * The OpenSpec composer. (M28)
          *
          * Here and not in the panel, which is where it used to be: there are three doors to it
          * now — the panel's two buttons and the `spec.propose` / `spec.explore` palette commands
          * — and two of them exist whether or not the sidebar is showing OpenSpec. A composer
          * mounted inside the panel would make the palette command work only when the surface it
          * was meant to replace was already on screen.
          *
          * Behind a boundary for the sidebar panels' reason: it is a portal, and a throw inside
          * it must not take the window down. Closing clears the store, which unmounts this and
          * thereby resets the boundary for the next open.
          */}
        <PanelBoundary name="OpenSpec compose" onClose={() => useProposeDialog.getState().close()}>
          <ProposeDialog project={activeProjectId} />
        </PanelBoundary>

        {/*
          * The switcher popup — one component for Ctrl+Tab (tabs) and Ctrl+` (projects). No
          * props and no keyboard handling: the walk, its claim on the walk key and on Escape,
          * and the release watcher all live in `keys/switcherStore.ts`, so the gesture already
          * works without this line — mounting it is what makes it *visible*. Without it a
          * switcher switches silently, which is the same "did anything happen?" failure this
          * project has now fixed a dozen times.
          */}
        <Switcher />

        {pendingClose && (
          <CloseConfirm
            scope={pendingClose.scope}
            unsaved={pendingClose.unsaved}
            sessions={pendingClose.sessions}
            onCancel={() => useCloseConfirm.getState().dismiss()}
            onDiscard={() => void useCloseConfirm.getState().confirm()}
            /*
             * Offered only when every unsaved file has a live editor that can write it.
             * A Save button that silently skipped a file it could not reach would lose
             * exactly what the dialog exists to protect, so `CloseConfirm` omits the button
             * entirely rather than showing one it cannot honour — passing `undefined` is
             * how that is said.
             */
            onSaveAll={
              canSaveAll(pendingClose.unsaved.map((u) => u.tab))
                ? () => {
                    const tabs = pendingClose.unsaved.map((u) => u.tab)
                    void saveAll(tabs).then(({ failed }) => {
                      if (failed.length > 0) {
                        // The close does NOT proceed. A write that did not land leaves the
                        // tab dirty, and Rust would refuse the close anyway — but stopping
                        // here is what keeps the user in front of the file rather than
                        // bouncing them off a second dialog.
                        void diag.log(`could not save ${failed.length} file(s); close cancelled`)
                        useCloseConfirm.getState().dismiss()
                        return
                      }
                      void useCloseConfirm.getState().confirm()
                    })
                  }
                : undefined
            }
          />
        )}

        {/*
          * The cursor slot used to carry `rev 0` — the workspace revision, a boot-time
          * debugging aid in the one place on screen reserved for the caret. The editor now
          * fills that end of the bar with the file readout it owns, and the bar subscribes
          * to it directly (see `chrome/StatusBar.tsx`), so nothing is passed for it here.
          */}
        <StatusBar
          claude={claudeReadout ?? boot?.capabilities.claudeVersion ?? undefined}
          revealRoots={revealRoots}
          /*
           * Whether the bar says anything about a file at all. The same expression
           * `keyContext.editorFocused` is built from, off the same `focused` const — which is
           * what that const exists for, and it is the status bar its doc names.
           *
           * Without it both file slots stand for ever: the readout claim stack has no demotion
           * (a terminal pane claims nothing, so nothing displaces the last buffer), so the path
           * and the caret position went on describing a file the user had clicked away from.
           */
          editorFocused={focused?.pane.kind === 'editor'}
          /*
           * A crumb of the open file's path trail, straight to `file.reveal` and to nothing
           * else. Identical routing to `Explorer`'s ⌖ button above and to a Ctrl+click on a
           * directory in terminal output, and for the identical reason: that arm brings the
           * sidebar to Files first, resolves an *External Libraries* group that has never been
           * opened, and reports a path with no row in a sentence — none of which is worth a
           * second copy here, and all of which a second reveal path would have to keep in step.
           *
           * **Withheld in a window with no sidebar**, which draws every crumb inert rather than
           * live-and-refusing. `dispatch.ts` answers `file.reveal` with a notice when
           * `role.kind !== 'shell'`, and a detached *tab* window reaches this line — the
           * detached-*pane* branch returns long before it. Same rule and same shape as
           * `onRevealPath` on the terminal above.
           */
          onRevealSegment={
            boot?.role.kind === 'shell'
              ? (path) => runCommand('file.reveal', { path })
              : undefined
          }
        />

        {/*
          * Says so when the IPC transport has silently degraded. WebKitGTK's custom-protocol
          * path can fail at boot, after which `ipc-protocol.js` sets `customProtocolIpcFailed`
          * permanently, logs one `console.warn` nobody sees on Wayland, and falls back to
          * string `postMessage` — throughput collapses with no crash and no Rust-side signal.
          * That is indistinguishable from "the app is just slow", which is exactly the report
          * this came out of. All state lives in `transportWatch`; this takes no props.
          */}
        <TransportNotice />

        {benchMode() && (
          <button className={styles.benchButton} onClick={onBench} disabled={benchRunning}>
            {benchRunning ? 'benchmarking…' : 'Run IPC bench (M0 gate)'}
          </button>
        )}

        {bench && (
          <div className={styles.bench} onClick={() => !benchRunning && setBench(null)}>
            <div className={styles.benchCard}>{bench}</div>
          </div>
        )}
      </div>
    </WindowFrame>
  )
}



/**
 * The pane grid and its tab strip — the workspace's centre — behind the frontend's one
 * load-bearing memo boundary.
 *
 * `App` subscribes to the whole `boot` and re-renders on every store notification: an
 * overlay opening, a diagnostics push, a context menu, `pendingClose`. Before this boundary
 * existed each of those walked every `PaneFrame`/`PaneBody`/editor/terminal frame of every
 * tab of the active project — `TabContent` deliberately keeps every tab mounted — so the
 * biggest subtree in the app re-rendered at the frequency of its noisiest neighbour. The
 * memo holds because every prop is either snapshot-derived (`activeProject`, `visibleTabs`,
 * `tabRole` — a new identity exactly once per accepted mutation, which is the one time the
 * grid should re-render) or pinned (`runCommand` and `openTerminalPath` are stable
 * `useCallback` wrappers, `restorePlan` and `plannedProjects` are `useState` values written
 * once per project as its plan lands, `tabWindow` is a window-lifetime constant). A new prop
 * added here must be one of those two things, or it silently reopens the hole this component
 * closes.
 *
 * The store *actions* are read here rather than passed as props: zustand actions are
 * created once, so these subscriptions never fire, and everything that can go stale still
 * arrives through the memoised props. In the same file as `App` (like `ExtSidebarPanel`
 * below) because several check scripts assert on this JSX by scanning `App.tsx`.
 */
const WorkspaceContent = memo(function WorkspaceContent({
  activeProject,
  settingsFrame,
  onCloseSettingsFrame,
  visibleTabs,
  tabRole,
  tabWindow,
  restorePlan,
  plannedProjects,
  runCommand,
  openTerminalPath,
}: {
  activeProject: Project | null
  /**
   * Whether the empty frame is showing the Settings screen. (M74)
   *
   * A boolean and a stable callback, so the memo above still holds — see its header for why a
   * new prop here has to be one of those two shapes.
   */
  settingsFrame: SettingsSection | null
  onCloseSettingsFrame: () => void
  visibleTabs: readonly Tab[]
  tabRole: Extract<WindowRole, { kind: 'detachedTab' }> | null
  tabWindow: boolean
  restorePlan: ReadonlyMap<string, PaneRestore>
  plannedProjects: ReadonlySet<string>
  runCommand: (command: string, args: unknown) => void
  openTerminalPath: (
    project: ProjectId,
  ) => (path: string, at: { line: number; column: number } | null) => void
}) {
  const activateTab = useWorkspace((s) => s.activateTab)
  const closeTab = useWorkspace((s) => s.closeTab)
  const closeTabs = useWorkspace((s) => s.closeTabs)
  const reorderTab = useWorkspace((s) => s.reorderTab)
  const splitPane = useWorkspace((s) => s.splitPane)
  const closePane = useWorkspace((s) => s.closePane)
  const focusPane = useWorkspace((s) => s.focusPane)
  const maximizePane = useWorkspace((s) => s.maximizePane)
  const movePane = useWorkspace((s) => s.movePane)
  const setRatio = useWorkspace((s) => s.setRatio)
  const bindSession = useWorkspace((s) => s.bindSession)
  const detachPane = useWorkspace((s) => s.detachPane)
  const detachTab = useWorkspace((s) => s.detachTab)

  // Hoisted out of `renderPane`, which built them per pane per render: both depend only on
  // the project.
  const rootPaths = useMemo(
    () => (activeProject === null ? [] : activeProject.roots.map((r) => r.path)),
    [activeProject],
  )
  const onOpenPath = useMemo(
    () => (activeProject === null ? undefined : openTerminalPath(activeProject.id)),
    [openTerminalPath, activeProject],
  )

  /*
   * The Settings screen with no project open. (M74)
   *
   * It stands where the tab strip and the tab content stand, and it is only reachable in the
   * one state where neither of those is drawn — `TabStrip` and `TabContent` below are both
   * gated on `activeProject`, so this is the empty work area filled rather than anything
   * covered up. The two mode flags are the same two the grid carries: `CIDE_AUDIT=1` measures
   * the chrome against a mock that has no such screen, and the IPC bench should not be racing a
   * settings probe.
   */
  if (!tabWindow && !auditMode() && !benchMode() && activeProject === null && settingsFrame) {
    return <SettingsFrame section={settingsFrame} onClose={onCloseSettingsFrame} />
  }

  return (
    <>
              {tabWindow ? null : auditMode() ? (
                <TabStrip tabs={AUDIT_TABS} activeTab={AUDIT_ACTIVE_TAB} />
              ) : (
                activeProject && (
                  <TabStrip
                    // The window's own view of the strip, not `activeProject.tabs`: a tab
                    // torn out into its own window keeps its slot in the domain's list, and
                    // drawing it here would be two windows claiming one tab.
                    tabs={visibleTabs}
                    activeTab={activeProject.activeTab}
                    onActivate={(id) => void activateTab(activeProject.id, id)}
                    onClose={(id) => void closeTab(activeProject.id, id)}
                    onCloseMany={(ids) => void closeTabs(activeProject.id, ids)}
                    onReorder={(id, before) => void reorderTab(activeProject.id, id, before)}
                    // Through `runCommand`, not straight to the store, and for the reason
                    // `onSelectOpened` routes the same way: the preconditions — is this a shell
                    // window, does the project hold a repository, which repository is this path in
                    // — live in one dispatch case, and a second copy in a menu handler is a second
                    // copy that stops matching.
                    onShowHistory={(path) => runCommand('git.history.file', { path })}
                    /*
                     * *Properties*, through `runCommand` for the reason `onShowHistory` beside
                     * it routes that way: the path resolution and every refusal sentence live in
                     * one dispatch case. The file tree's copy of this row calls the opener
                     * directly instead, because it already holds a path the tree resolved — both
                     * ends land on `openFileProperties`.
                     */
                    onProperties={(path) => runCommand('file.properties', { path })}
                    onSplit={() => {
                      // Splits the focused pane sideways with the tab's default intent —
                      // a shell in the pinned console, a new session in a ClaudeFull tab.
                      // Which intent that is belongs to the domain, so `null` asks for it.
                      const tab = activeProject.tabs.find((t) => t.id === activeProject.activeTab)
                      if (!tab) return
                      void splitPane(activeProject.id, tab.id, tab.tree.focused, 'row', 'after')
                    }}
                  />
                )
              )}

              {/* No terminals during a benchmark run: four live PTYs competing for the same
                  IPC channel would be measuring the wrong thing. The chrome audit skips them
                  for a different reason — it measures chrome, and four `claude` processes are
                  a slow way to take a ruler to a status bar. */}
              {/* `plannedProjects` gates the whole tree, not just the prop: a pane that
                  renders before its project's plan lands latches `PaneBody`'s splash decision
                  without it. See the note on the state itself. */}
              {!benchMode() &&
                !auditMode() &&
                activeProject &&
                plannedProjects.has(activeProject.id) && (
                <TabContent
                  // The same filtered list the strip draws — `windows/windowTabs.ts` — and
                  // here it is the load-bearing copy: `TabContent` MOUNTS every tab it is
                  // handed, hidden or not, so an unfiltered list would put a second live
                  // editor (and a second buffer) behind the shell for every torn-out file.
                  tabs={visibleTabs}
                  // A `tab:` window's one tab comes from its role; the project's
                  // `activeTab` deliberately points at a tab the *shell* draws.
                  activeTab={tabRole !== null ? tabRole.tab : activeProject.activeTab}
                  renderTree={(tab, active) =>
                    // A settings tab has a pane tree — every tab does, which is the invariant
                    // that makes "promote pane to tab" one code path — but nothing to render
                    // into it. The screen replaces the tree rather than living inside a pane.
                    tab.kind.kind === 'settings' ? (
                      <SettingsTab project={activeProject.id} section={tab.kind.section} />
                    ) : tab.kind.kind === 'extension' ? (
                      /*
                       * An extension's page, replacing the pane tree the way Settings does — and
                       * for the same reason: it is a page *about* cide rather than a document in
                       * the project, so there is nothing for a split to split.
                       */
                      <ExtensionTab
                        id={{ marketplace: tab.kind.marketplace, extension: tab.kind.extension }}
                        name={tab.kind.name}
                      />
                    ) : tab.kind.kind === 'openSpec' ? (
                      /*
                       * A change or a capability, replacing the pane tree for the two reasons
                       * above: it is a page about a *directory* rather than a document, and
                       * there is nothing for a split to split. `TabKind::OpenSpec` argues why
                       * opening `proposal.md` was not enough.
                       */
                      <SpecTab
                        project={activeProject.id}
                        /*
                         * **Every tab is mounted**, hidden or not — `TabContent`'s own rule, and
                         * hidden means `visibility: hidden` rather than unmounted. So a page that
                         * bound a window-level key listener would bind one per open change and
                         * they would all fire at once. `active` is what tells this one the chord
                         * is meant for it.
                         */
                        active={active}
                        {...(tab.kind.subject.kind === 'change'
                          ? { change: tab.kind.subject.change }
                          : { spec: tab.kind.subject.spec })}
                      />
                    ) : tab.kind.kind === 'docs' ? (
                      /*
                       * A symbol's documentation, replacing the pane tree for the reasons the
                       * arms around it give: a page *about* something, nothing for a split to
                       * split. The tab carries a subject; the pane asks for the page. (M60)
                       */
                      <DocsPane
                        project={activeProject.id}
                        subject={tab.kind.subject}
                        title={tab.kind.title}
                      />
                    ) : tab.kind.kind === 'docker' ? (
                      /*
                       * A `docker inspect` document, replacing the pane tree for the two reasons
                       * the OpenSpec and Extension arms above give: it is a page *about*
                       * something rather than a document in the project, and there is nothing
                       * for a split to split.
                       */
                      <DockerInspectPane target={tab.kind.target} title={tab.kind.title} />
                    ) : tab.kind.kind === 'dockerFiles' ? (
                      <DockerFilesPane container={tab.kind.container} name={tab.kind.name} />
                    ) : (
                    <SplitTree
                      tree={tab.tree}
                      onFocus={(id) => void focusPane(activeProject.id, tab.id, id)}
                      onRatioCommit={(split, ratio) =>
                        void setRatio(activeProject.id, tab.id, split, ratio)
                      }
                      renderPane={(paneNode, index) => {
                        /*
                         * What this pane's corner cluster acts on — `windows/windowTabs.ts`
                         * holds the rule. A tab's only pane closes and detaches as the
                         * *tab*: `layout::close` and `layout::take_pane` both refuse a
                         * tab's last pane, and these two buttons used to run straight into
                         * that refusal and print `lastPane` at the user. Its maximize is
                         * withheld outright — one pane already fills its tab. The pinned
                         * console is exempt so its primary pane keeps the role-based
                         * refusals `PaneFrame` already words.
                         *
                         * (`clusterPlan` reads the tab, not the pane, and is cheap enough
                         * that a per-pane call costs only tidiness — it stays here because
                         * this comment is about *what the plan means*, and the callback is
                         * where a reader meets it.)
                         */
                        const plan = clusterPlan(
                          Object.keys(tab.tree.panes).length,
                          activeProject.tabs[0]?.id === tab.id,
                        )
                        const tabScoped = plan.close === 'tab'
                        return (
                        <PaneFrame
                          pane={paneNode}
                          index={index}
                          focused={tab.tree.focused === paneNode.id}
                          maximized={tab.tree.maximized === paneNode.id}
                          tabScoped={tabScoped}
                          onFocus={() => void focusPane(activeProject.id, tab.id, paneNode.id)}
                          // `row` is a tile beside this one, in this pane's own row — the axis
                          // the command layer routes to `add_tile`. The row gesture is on the
                          // tree, not here, because it is not about any one pane.
                          onAddTile={() =>
                            void splitPane(activeProject.id, tab.id, paneNode.id, 'row', 'after')
                          }
                          // Both were drawn disabled, and that mattered more once the pane's
                          // title bar was deleted: the right-click menu became the primary route
                          // to splitting, so two of its items naming "use the header instead"
                          // was most of the gesture missing.
                          onSplitDown={() =>
                            void splitPane(activeProject.id, tab.id, paneNode.id, 'col', 'after')
                          }
                          onAddRow={() =>
                            void splitPane(activeProject.id, tab.id, tab.tree.focused, 'col', 'after')
                          }
                          /*
                           * The grab handle's commit. Withheld for a tab's only pane — there
                           * is nowhere to move it to — and `PaneFrame` withholds it again for
                           * a pane that is not a terminal, which is the kind gate.
                           *
                           * The DOM focus has to be put back by hand: the move re-parents the
                           * pane into a different chain grid, so React unmounts its slot and
                           * `parkHost` blurs the terminal on the way out. Without this the
                           * ring lands on the moved pane and the keystrokes go nowhere — the
                           * defect `panes/paneFocus.ts` was written about.
                           */
                          onMove={
                            plan.move
                              ? (outcome) => {
                                  void movePane(
                                    activeProject.id,
                                    tab.id,
                                    outcome.pane,
                                    outcome.target,
                                    outcome.axis,
                                    outcome.side,
                                  ).then(() => {
                                    focusPaneDom(outcome.pane)
                                  })
                                }
                              : undefined
                          }
                          onMaximize={
                            plan.maximize
                              ? () =>
                                  void maximizePane(
                                    activeProject.id,
                                    tab.id,
                                    tab.tree.maximized === paneNode.id ? null : paneNode.id,
                                  )
                              : undefined
                          }
                          onDetach={
                            plan.detach === 'tab'
                              ? // A tab already in a window of its own has nowhere further
                                // out to go; withholding the handler hides the button and
                                // greys the menu row with that sentence.
                                tabWindow
                                ? undefined
                                : () => void detachTab(activeProject.id, tab.id)
                              : () => void detachPane(activeProject.id, tab.id, paneNode.id)
                          }
                          onClose={
                            plan.close === 'tab'
                              ? // Through `closeTab`, which asks about a live session or an
                                // unsaved buffer before it discards either — the same dialog
                                // the strip's × raises for this tab.
                                () => void closeTab(activeProject.id, tab.id)
                              : () => void closePane(activeProject.id, tab.id, paneNode.id)
                          }
                        >
                          {/*
                            * A git diff replaces the pane rather than living in one, the way a
                            * settings tab does. A Claude diff is different and stays in
                            * `PaneBody`: it is answering a blocked agent turn and belongs
                            * beside the terminal that is waiting on it.
                            */}
                          {tab.kind.kind === 'diff' && tab.kind.spec.origin.kind === 'git' ? (
                            /*
                             * `visible` is passed, and the pane's own fallback is the safety
                             * net rather than the mechanism. `diffTabs.diffTabOnScreen` exists
                             * because a deferral switched on by a prop is a deferral that is
                             * off — but it can only answer once a `cide://workspace-changed`
                             * has arrived, and `onScreen` starts `true`, so a workspace
                             * restored with six diff tabs had all six believing they were in
                             * front until the first mutation. That window is where the boot
                             * storm lived: six `git_diff_file` calls, six whole-file wires,
                             * six tokenizes. `active` is the flag `TabContent` already hands
                             * this closure, and the line below hands it to `PaneBody`.
                             */
                            <GitDiffPane
                              project={activeProject.id}
                              spec={tab.kind.spec}
                              visible={active}
                            />
                          ) : (
                          <PaneBody
                            pane={paneNode}
                            // The run's worktree for a pane opened onto a run's conversation
                            // (M42); the project root for every other pane.
                            cwd={paneNode.continues?.cwd ?? activeProject.roots[0]?.path ?? '.'}
                            project={activeProject.id}
                            primarySession={activeProject.primarySession}
                            diff={tab.kind.kind === 'diff' ? tab.kind.spec : undefined}
                            editor={
                              tab.kind.kind === 'file'
                                ? { tab: tab.id, path: tab.kind.path }
                                : undefined
                            }
                            restore={restorePlan.get(paneNode.id)}
                            roots={rootPaths}
                            onOpenPath={onOpenPath}
                            /*
                             * A ctrl+click on a *directory* in terminal output.
                             *
                             * Straight to `file.reveal` and to nothing else, so the sidebar is
                             * brought to Files first and a path with no row reports itself in a
                             * sentence — both of which are that arm's rules, and neither of which
                             * is worth a second copy here. It is the same routing
                             * `Explorer`'s ⌖ button makes (`onSelectOpened` above), for the same
                             * reason: a second call site with its own preconditions is how three
                             * gestures come to behave in three ways.
                             *
                             * Passed only in the shell branch. The detached-pane window above has
                             * no sidebar, so it passes no handler, and `pathLinks.ts` then draws
                             * no directory underline there at all rather than one whose command
                             * would refuse.
                             */
                            onRevealPath={(path) => runCommand('file.reveal', { path })}
                            onSessionBound={(session) =>
                              void bindSession(activeProject.id, tab.id, paneNode.id, session)
                            }
                            /*
                             * Which tab is in front, which `TabContent` computes and has offered
                             * through this callback's second argument since M4 — and which this
                             * call site wrote `(tab) =>` and discarded.
                             *
                             * The editor is the only pane kind that needs it, and needs it because
                             * the hiding here is pure CSS: a background tab's panes are mounted,
                             * laid out at full size and painting, so nothing below can tell them
                             * from the visible one. Without it the status bar's file readout was
                             * claimed by whichever restored tab's `file_read` resolved last, and a
                             * Ctrl+Tab moved nothing. See `editor/statusReadout.ts`.
                             */
                            onScreen={active}
                          />
                          )}
                        </PaneFrame>
                        )
                      }}
                    />
                    )
                  }
                />
              )}
    </>
  )
})


/**
 * One contributed sidebar panel, resolved from the rail's view id. (M22)
 *
 * A component rather than an inline `find`, because the lookup can fail in a way worth drawing: a
 * panel whose extension was disabled in *another window* leaves this window's rail selection
 * pointing at nothing, and the honest answer is a sentence rather than a blank column. `PanelView`
 * is restored from `sidebar.last` on the next gesture.
 */
function ExtSidebarPanel({
  view,
  panels,
}: {
  view: string
  panels: readonly PanelBinding[]
}): React.JSX.Element {
  const binding = panels.find((panel) => panel.view === view)
  if (binding === undefined) {
    return (
      <div className={styles.extMissing}>
        That panel is not available — its extension was disabled or removed.
      </div>
    )
  }
  return <ExtPanelHost binding={binding} />
}
