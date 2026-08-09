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
import { useCallback, useEffect, useState } from 'react'
import { AppHeader } from '@/chrome/AppHeader'
import { ActivityRail, type ActivityView } from '@/chrome/ActivityRail'
import { StatusBar } from '@/chrome/StatusBar'
import { TabStrip } from '@/chrome/TabStrip'
import { WindowFrame } from '@/chrome/WindowFrame'
import { auditMode, formatAuditReport, runLayoutAudit } from '@/chrome/layoutAudit'
import { AUDIT_ACTIVE_TAB, AUDIT_TABS } from '@/chrome/auditFixture'
import { auditPanesMode, formatPaneAudit, runPaneAudit } from '@/layout/paneAudit'
import { createAppPaneDriver } from '@/layout/appPaneDriver'
import { auditWindowsMode, formatWindowAudit, runWindowAudit } from '@/layout/windowAudit'
import { createAppWindowDriver } from '@/layout/appWindowDriver'
import { DetachedPaneWindow } from '@/windows/DetachedPaneWindow'
import { SplitTree } from '@/layout/SplitTree'
import { TabContent } from '@/layout/TabContent'
import { PaneFrame } from '@/layout/PaneTitleBar'
import { PaneBody } from '@/panes/PaneBody'
import { liveHosts } from '@/layout/paneHosts'
import { open as openDialog } from '@tauri-apps/plugin-dialog'
import {
  app as appApi,
  benchMode,
  diag,
  type PaneRestore,
  type ProjectId,
  type TabId,
} from '@/ipc/client'
import { runBench, formatReport } from '@/bench/ipcBench'
import { retheme } from '@/terminal/xterm'
import { useWorkspace } from '@/store/workspace'
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

export function App() {
  const boot = useWorkspace((s) => s.boot)
  const theme = useWorkspace((s) => s.theme)
  const toggleTheme = useWorkspace((s) => s.toggleTheme)
  const hydrate = useWorkspace((s) => s.hydrate)
  const openProject = useWorkspace((s) => s.openProject)
  const closeProject = useWorkspace((s) => s.closeProject)
  const activateTab = useWorkspace((s) => s.activateTab)
  const closeTab = useWorkspace((s) => s.closeTab)

  const splitPane = useWorkspace((st) => st.splitPane)
  const closePane = useWorkspace((st) => st.closePane)
  const focusPane = useWorkspace((st) => st.focusPane)
  const maximizePane = useWorkspace((st) => st.maximizePane)
  const setRatio = useWorkspace((st) => st.setRatio)
  const bindSession = useWorkspace((st) => st.bindSession)
  const newClaudeTab = useWorkspace((st) => st.newClaudeTab)
  const detachPane = useWorkspace((st) => st.detachPane)
  const redockPane = useWorkspace((st) => st.redockPane)

  const [view, setView] = useState<ActivityView>('files')
  /**
   * The launch plan, keyed by pane.
   *
   * Read once and never refreshed: it describes what the workspace looked like when this
   * process started, so a pane created later has no entry and spawns immediately — which is
   * correct, because the user just asked for it.
   */
  const [restorePlan, setRestorePlan] = useState<Map<string, PaneRestore>>(new Map())
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

  useEffect(() => {
    void appApi
      .restorePlan()
      .then((plan) => setRestorePlan(new Map(plan.map((e) => [e.pane, e]))))
      .catch((e) => diag.log(`restore plan unavailable: ${String(e)}`))
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

  // A `pane:<uuid>` window shows exactly one pane. It shares the workspace mirror with the
  // shell window but none of its chrome: no project tabs, no rail, no status bar.
  if (boot?.role.kind === 'detachedPane') {
    const { project, pane } = boot.role
    const owner = boot.workspace.projects[project]
    const detachedPane = owner?.detached[pane]
    if (!owner || !detachedPane) {
      // The domain no longer holds this pane — its project closed while the window was up.
      // Rendering nothing is honest; the window closes on the next snapshot.
      return <WindowFrame>{null}</WindowFrame>
    }
    return (
      <DetachedPaneWindow
        pane={detachedPane}
        cwd={owner.roots[0]?.path ?? PROJECT_ROOT}
        project={project}
        onRedock={() => void redockPane(boot.window)}
      />
    )
  }

  return (
    <WindowFrame>
      <div className={styles.app}>
        <AppHeader
          projects={projects}
          activeProject={activeProjectId}
          onClose={(id) => void closeProject(id)}
          onNew={() => void pickProject(openProject)}
          onToggleTheme={toggleTheme}
        />

        <div className={styles.body}>
          {/* The badge only renders on a dirty tree, so the audit has to ask for one or it
              measures a state the chrome never shows it. */}
          <ActivityRail active={view} gitDirty={auditMode()} onSelect={setView} />

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
            {!benchMode() && !auditMode() && activeProject && (
              <TabContent
                tabs={activeProject.tabs}
                activeTab={activeProject.activeTab}
                renderTree={(tab) => (
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
                        <PaneBody
                          pane={paneNode}
                          cwd={activeProject.roots[0]?.path ?? PROJECT_ROOT}
                          project={activeProject.id}
                          diff={tab.kind.kind === 'diff' ? tab.kind.spec : undefined}
                          restore={restorePlan.get(paneNode.id)}
                          onSessionBound={(session) =>
                            void bindSession(activeProject.id, tab.id, paneNode.id, session)
                          }
                        />
                      </PaneFrame>
                    )}
                  />
                )}
              />
            )}
          </div>
        </div>

        <StatusBar
          claude={boot?.capabilities.claudeVersion ?? undefined}
          cursor={`rev ${boot?.workspace.rev ?? 0}`}
        />

        {!auditMode() && (
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
