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
import { useCallback, useEffect, useMemo, useState } from 'react'
import { AppHeader } from '@/chrome/AppHeader'
import { ActivityRail } from '@/chrome/ActivityRail'
import {
  SIDEBAR_INITIAL,
  isPanelOpen,
  selectView,
  showPanel,
  toggleSidebar,
  type SidebarState,
} from '@/chrome/sidebarView'
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
import { TabContent } from '@/layout/TabContent'
import { Explorer } from '@/sidebar/Explorer'
import { GitPanel } from '@/sidebar/GitPanel'
import { SearchPanel } from '@/sidebar/SearchPanel'
import { ProblemsPanel } from '@/sidebar/ProblemsPanel'
import { ALL_VISIBLE, applyFilters, statusBarCounts } from '@/sidebar/ProblemsPanel/model'
import { useDiagnostics } from '@/sidebar/diagnosticsStore'
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
import { requestOutsideOpen } from '@/chrome/outsideOpenStore'
import { outsideAsk } from '@/terminal/outsideOpen'
import { canSaveAll, saveAll } from '@/editor/openBuffers'
import { revealPane } from '@/editor/revealPane'
import { jumpTo, pendingJump } from '@/editor/jump'
import { UNKNOWN_LINE } from '@/editor/navHistory'
import { claudeSend } from '@/ipc/client'
import { useKeyGate } from '@/keys/useKeyGate'
import { createDispatcher } from '@/keys/dispatch'
import { buildKeymap } from '@/keys/keymap'
import { PaneFrame } from '@/layout/PaneTitleBar'
import { PaneBody } from '@/panes/PaneBody'
import { GitDiffPane } from '@/panes/GitDiffPane'
import { liveHosts } from '@/layout/paneHosts'
import { SettingsTab } from '@/settings/SettingsTab'
import { toggleTheme as togglePersistedTheme } from '@/settings/useSettings'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import {
  app as appApi,
  benchMode,
  file as fileApi,
  fs as fsApi,
  project as projectApi,
  windows as windowApi,
  diag,
  events,
  settings as settingsApi,
  type PaneRestore,
  type ProjectId,
  type SplitIntent,
  type TabId,
} from '@/ipc/client'
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

const PROJECT_ROOT = '/home/lantian/work/cide'

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
    await openProject([PROJECT_ROOT])
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

/** The viewport the design mock is drawn at, and the one the audit must measure in. */
const MOCK_WIDTH = 1440
const MOCK_HEIGHT = 900

/**
 * Ask for a directory and open it as a project.
 *
 * The native picker rather than a text field: a project is a real path, and typing one is
 * both slower and the only way to get it wrong.
 */
async function pickProject(open: (paths: string[]) => Promise<void>): Promise<void> {
  const chosen = await openDialog({ directory: true, multiple: true, title: 'Open project' })
  if (chosen === null) return
  await open(Array.isArray(chosen) ? chosen : [chosen])
}

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
  const closeProject = useWorkspace((s) => s.closeProject)
  const activateTab = useWorkspace((s) => s.activateTab)
  const closeTab = useWorkspace((s) => s.closeTab)
  const closeTabs = useWorkspace((s) => s.closeTabs)
  const reorderTab = useWorkspace((s) => s.reorderTab)

  const splitPane = useWorkspace((st) => st.splitPane)
  const closePane = useWorkspace((st) => st.closePane)
  const focusPane = useWorkspace((st) => st.focusPane)
  const maximizePane = useWorkspace((st) => st.maximizePane)
  const contextMenuOpen = useContextMenuOpen()
  const setRatio = useWorkspace((st) => st.setRatio)
  const bindSession = useWorkspace((st) => st.bindSession)
  const newClaudeTab = useWorkspace((st) => st.newClaudeTab)
  const detachPane = useWorkspace((st) => st.detachPane)
  const redockPane = useWorkspace((st) => st.redockPane)

  /**
   * Which sidebar view the rail has lit, whether a panel is showing, and which one comes back.
   *
   * `view: null` is a real state and always was: clicking the lit rail button hides its panel,
   * so the workspace can have the full window width without dragging the splitter to the edge.
   * What it had no way to be was *reached without a mouse*, and no memory of what to restore —
   * see `chrome/sidebarView.ts`, which owns every rule about this value so that a check script
   * can compile them. Nothing here is persisted; that module says why.
   */
  const [sidebar, setSidebar] = useState<SidebarState>(SIDEBAR_INITIAL)
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
   * The launch plan, keyed by pane.
   *
   * Read once and never refreshed: it describes what the workspace looked like when this
   * process started, so a pane created later has no entry and spawns immediately — which is
   * correct, because the user just asked for it.
   *
   * **`null` means "not fetched yet", and no pane is rendered until it is not `null`.** It used
   * to start as an empty map, which is indistinguishable from "the plan says nothing about any
   * of these panes" — and that is a different claim with two consequences, both silent.
   * `PaneBody` latches its Resume splash on the *first* render (the splash and the terminal are
   * different element types in one position, so it cannot be recomputed), so a tree painted
   * before the plan arrived spawned every restored Claude pane at once — the "reopening a
   * six-pane project starts six agents" case the splash exists to prevent. And a pane with no
   * entry never passes `--resume`, so the conversation comes back empty. Both depended on two
   * unrelated mount effects resolving in the order they were declared.
   *
   * A failed fetch sets an empty map rather than leaving this `null`: a plan that cannot be
   * read costs a pane its resume, and a workspace that never renders costs the user everything.
   */
  const [restorePlan, setRestorePlan] = useState<Map<string, PaneRestore> | null>(null)
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
    void appApi
      .restorePlan()
      .then((plan) => setRestorePlan(new Map(plan.map((e) => [e.pane, e]))))
      .catch((e) => {
        // An empty plan, not a permanent `null`: every pane then spawns fresh, which is the
        // same thing this window did before the plan existed. Leaving it `null` would hold the
        // whole workspace off the screen because one optimisation could not be read.
        setRestorePlan(new Map())
        return diag.log(`restore plan unavailable: ${String(e)}`)
      })
  }, [])

  useEffect(() => {
    void appApi.ready()
    void hydrate()
      .then(async () => {
        // The chrome audit needs a project tab and a tab strip, neither of which exists in
        // an empty workspace. The pane audit seeds itself, in sequence — see below.
        if (!auditMode()) return
        if (Object.keys(useWorkspace.getState().boot?.workspace.projects ?? {}).length > 0) return
        await openProject([PROJECT_ROOT])
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
      await seedAuditWorkspace(openProject, newClaudeTab)
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

  const projects = boot ? Object.values(boot.workspace.projects) : []
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
  }

  /*
   * The one dispatcher. The key gate and the command palette both call it, which is what
   * stops a binding and its palette entry drifting into two different behaviours.
   */
  const runCommand = createDispatcher({
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
    showSidebar: (next) => setSidebar((s) => showPanel(s, next)),
    /*
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
  const statusBySession = useSessionStatus((s) => s.bySession)
  const claudeReadout = focusedSession
    ? formatClaude(statusBySession[focusedSession])
    : undefined

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
              // Before the hydrate, so the entry is on the stack by the time anything can render
              // — and after the open, so a refusal never becomes a place the user has been.
              arrived?.()
              hydrate()
            })
            .catch((reason: unknown) => {
              const ask = outsideAsk(reason)
              if (ask === null) return notifyFailure(reason)
              requestOutsideOpen({ ask, proceed: () => attempt(ask.target) })
            })
        }
        attempt()
      },
    [hydrate],
  )

  // A `pane:<uuid>` window shows exactly one pane. It shares the workspace mirror with the
  // shell window but none of its chrome: no project tabs, no rail, no status bar.
  if (boot?.role.kind === 'detachedPane') {
    const { project, tab: homeTab, pane } = boot.role
    const owner = boot.workspace.projects[project]
    const detachedPane = owner?.detached[pane]
    // Nothing until the plan is in, for the reason on `restorePlan` above. This window is the
    // one that suffered most from doing otherwise: it never received a plan at all, so a
    // torn-out pane came back at launch, adopted a `SessionId` from the process that had
    // written `workspace.json`, and printed `— no such session —` into a blank pane every time.
    if (restorePlan === null) return <WindowFrame>{null}</WindowFrame>
    if (!owner || !detachedPane) {
      // The domain no longer holds this pane — its project closed while the window was up.
      // Rendering nothing is honest; the window closes on the next snapshot.
      return <WindowFrame>{null}</WindowFrame>
    }
    return (
      <>
        <DetachedPaneWindow
          pane={detachedPane}
          cwd={owner.roots[0]?.path ?? PROJECT_ROOT}
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
      </>
    )
  }

  return (
    <WindowFrame>
      <div className={styles.app}>
        <AppHeader
          projects={projects}
          activeProject={activeProjectId}
          /*
           * Clicking a project tab did nothing until now. `activate_project` has been in the
           * domain since projects went multi-window and no command exposed it, so with two
           * projects open there was no way to switch between them — the same dead control
           * the sidebar's search button was.
           */
          onActivate={(id) => void projectApi.activate(id as ProjectId).then(() => hydrate())}
          onClose={(id) => void closeProject(id)}
          onNew={() => void pickProject(openProject)}
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

        <div className={styles.body}>
          {/*
            * The badge is withheld at zero — a clean tree needs no ornament — so the audit has
            * to ask for a count or it measures a state the chrome never shows it. That flag
            * used to be the *only* thing feeding this prop: `gitDirty={auditMode()}` wired the
            * badge to a query parameter and to nothing else, so outside `?audit=1` it never
            * appeared in any repository state. The count is now real and the fixture is the
            * override, which is the way round every other audit-driven surface here works.
            */}
          <ActivityRail
            active={sidebar.view}
            changed={auditMode() ? AUDIT_GIT_CHANGES : gitChanged}
            errors={statusBarCounts(diagnostics.snapshot)?.errors ?? null}
            onSelect={(next) => {
              // What a click means — the lit one toggles shut, any other switches, ⚙ has no
              // panel to hide — is `selectView`'s, in `chrome/sidebarView.ts`, along with the
              // memory of which panel F4 restores. What is left here is the side effect only
              // this component can perform: Settings is a workspace *tab* rather than a
              // sidebar view, so choosing it also opens (or re-activates) the project's tab.
              setSidebar((s) => selectView(s, next))
              if (next === 'settings' && activeProjectId) {
                void settingsApi.openTab(activeProjectId).then(() => hydrate())
              }
            }}
          />

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
              onSelectOpened={() => runCommand('file.reveal', null)}
              onOpenFile={(path) => {
                if (!activeProjectId) return
                // `UNKNOWN_LINE`: the Explorer names a file and not a place in it, so the
                // per-file view memory decides where it opens. The history records the visit
                // without inventing a line — see `editor/navHistory.ts`.
                jumpTo(activeProjectId, { path, line: UNKNOWN_LINE, column: 1 })
                void fileApi.open(activeProjectId, path).then(() => hydrate())
              }}
            />
          )}
          {sidebar.view === 'git' && <GitPanel project={activeProjectId} />}
          {sidebar.view === 'search' && (
            <SearchPanel
              project={activeProjectId}
              onOpenHit={(path, line, column, endColumn) => {
                if (!activeProjectId) return
                // The caret, which is the half the user reported missing — a search result
                // that opens the file at the top has not gone to the found place. `jumpTo`
                // also records the visit, so the mouse's Back button returns here.
                //
                // Requested BEFORE the open, and that ordering is the design rather than a
                // preference: the editor for this path usually does not exist yet, so the
                // request is parked and spent by the mount the `file.open` below causes. A
                // file already open is revealed immediately instead. See
                // `editor/revealRequest.ts` — the parked request expires, so one for a file
                // that never opens cannot fire when the user opens it by hand an hour later.
                jumpTo(activeProjectId, { path, line, column, endColumn })
                void fileApi.open(activeProjectId, path).then(() => hydrate())
              }}
            />
          )}
          {sidebar.view === 'problems' && (
            <ProblemsPanel
              project={activeProjectId}
              snapshot={diagnostics.snapshot}
              hidden={diagnostics.hidden}
              onOpenLocation={(path, line, column) => {
                if (!activeProjectId) return
                // Requested BEFORE the open — the editor for this path usually does not exist
                // yet, so the request is parked and spent by the mount the open causes. The same
                // ordering the search panel's `onOpenHit` uses; see `editor/revealRequest.ts`.
                jumpTo(activeProjectId, { path, line, column })
                void fileApi.open(activeProjectId, path).then(() => hydrate())
              }}
            />
          )}
          {/* The sidebar's drag edge — one handle for all four panels, because there are only
              two widths: `--w-sidebar-files` sizes the explorer, search and problems, and
              `--w-sidebar-git` sizes git. Absent under `settings`, which has no panel, and
              while the sidebar is hidden: a handle with nothing to its left is a grab that
              resizes something the user cannot see. */}
          {isPanelOpen(sidebar) && (
            <SidebarSplitter panel={sidebar.view === 'git' ? 'git' : 'files'} />
          )}

          <div className={styles.content}>
            {/* An open project always has a console tab, so its `activeTab` is always live —
                but only once a project exists at all. */}
            {/* A real project opens with only its console tab, so a live app never renders
                a file tab or a language badge. The fixture covers every shape the strip has
                to get right; see chrome/auditFixture.ts. */}
            {auditMode() ? (
              <TabStrip tabs={AUDIT_TABS} activeTab={AUDIT_ACTIVE_TAB} />
            ) : (
              activeProject && (
                <TabStrip
                  tabs={activeProject.tabs}
                  activeTab={activeProject.activeTab}
                  onActivate={(id) => void activateTab(activeProject.id, id)}
                  onClose={(id) => void closeTab(activeProject.id, id)}
                  onCloseMany={(ids) => void closeTabs(activeProject.id, ids)}
                  onReorder={(id, before) => void reorderTab(activeProject.id, id, before)}
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
            {/* `restorePlan !== null` gates the whole tree, not just the prop: a pane that
                renders before the plan lands latches `PaneBody`'s splash decision without it.
                See the note on the state itself. */}
            {!benchMode() && !auditMode() && activeProject && restorePlan !== null && (
              <TabContent
                tabs={activeProject.tabs}
                activeTab={activeProject.activeTab}
                renderTree={(tab) =>
                  // A settings tab has a pane tree — every tab does, which is the invariant
                  // that makes "promote pane to tab" one code path — but nothing to render
                  // into it. The screen replaces the tree rather than living inside a pane.
                  tab.kind.kind === 'settings' ? (
                    <SettingsTab project={activeProject.id} section={tab.kind.section} />
                  ) : (
                  <SplitTree
                    tree={tab.tree}
                    onFocus={(id) => void focusPane(activeProject.id, tab.id, id)}
                    onRatioCommit={(split, ratio) =>
                      void setRatio(activeProject.id, tab.id, split, ratio)
                    }
                    renderPane={(paneNode, index) => (
                      <PaneFrame
                        pane={paneNode}
                        index={index}
                        focused={tab.tree.focused === paneNode.id}
                        maximized={tab.tree.maximized === paneNode.id}
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
                        onMaximize={() =>
                          void maximizePane(
                            activeProject.id,
                            tab.id,
                            tab.tree.maximized === paneNode.id ? null : paneNode.id,
                          )
                        }
                        onDetach={() => void detachPane(activeProject.id, tab.id, paneNode.id)}
                        onClose={() => void closePane(activeProject.id, tab.id, paneNode.id)}
                      >
                        {/*
                          * A git diff replaces the pane rather than living in one, the way a
                          * settings tab does. A Claude diff is different and stays in
                          * `PaneBody`: it is answering a blocked agent turn and belongs
                          * beside the terminal that is waiting on it.
                          */}
                        {tab.kind.kind === 'diff' && tab.kind.spec.origin.kind === 'git' ? (
                          <GitDiffPane project={activeProject.id} spec={tab.kind.spec} />
                        ) : (
                        <PaneBody
                          pane={paneNode}
                          cwd={activeProject.roots[0]?.path ?? PROJECT_ROOT}
                          project={activeProject.id}
                          primarySession={activeProject.primarySession}
                          diff={tab.kind.kind === 'diff' ? tab.kind.spec : undefined}
                          editor={
                            tab.kind.kind === 'file'
                              ? { tab: tab.id, path: tab.kind.path }
                              : undefined
                          }
                          restore={restorePlan.get(paneNode.id)}
                          roots={activeProject.roots.map((r) => r.path)}
                          onOpenPath={openTerminalPath(activeProject.id)}
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
                        />
                        )}
                      </PaneFrame>
                    )}
                  />
                  )
                }
              />
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
                void fileApi.open(activeProjectId, path).then(() => hydrate())
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
                void fileApi.open(activeProjectId, path).then(() => hydrate())
              },
              openFileInSplit: (path) => {
                closeOverlay()
                jumpTo(activeProjectId, { path, line: UNKNOWN_LINE, column: 1 })
                void fileApi.open(activeProjectId, path).then(() => hydrate())
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
                  await fileApi.open(activeProjectId, path)
                  hydrate()
                  setSidebar((s) => showPanel(s, 'files'))
                  const shown = await useFileTree.getState().reveal(path)
                  if (!shown) {
                    notify('The scratch file was created but has no row in the tree yet.', {
                      kind: 'info',
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
          * Last in the shell, so it paints over everything. A failed command used to be
          * indistinguishable from a control wired to nothing — which is precisely how the
          * `+ row` buttons presented when the running binary predated their command. See
          * `Failures.tsx`; it needs no props because it listens for the rejections the
          * `void` call sites above it discard.
          */}
        <Failures />

        {/* The out-of-project confirmation. Also in the detached branch above — one gesture,
            two window kinds, and neither of them may be the one that asks nobody. */}
        <OutsideOpenGate />

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
          /* The *filtered* snapshot, so the bar and the panel cannot disagree — and `null`
             whenever nothing has looked, which is what makes it print `✗ — ⚠ —`. */
          diagnostics={statusBarCounts(diagnostics.snapshot)}
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
