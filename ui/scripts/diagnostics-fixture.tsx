// Mounted diagnostics audit: real controls, menu, bottom host, key gate and GitLab store.
import React from 'react'
import { createRoot } from 'react-dom/client'
import { installFakeTauri } from '@/demo/fakeTauri'
import { useWorkspace } from '@/store/workspace'
import { ToolWindowHost } from '@/toolwindow/ToolWindowHost'
import { DiagnosticsStatus } from '@/chrome/DiagnosticsStatus'
import { StatusBar } from '@/chrome/StatusBar'
import { claimStatusReadout } from '@/editor/statusReadout'
import { ActivityRail } from '@/chrome/ActivityRail'
import { languageServers, toolWindow, searchSessions, clipboard, git, branch } from '@/ipc/client'
import { useToolSelection, showProblems, selectTransientToolTab } from '@/toolwindow/selection'
import { useProblemsState } from '@/toolwindow/problemsState'
import { createDispatcher } from '@/keys/dispatch'
import { installKeyGate } from '@/keys/gate'
import { registerPanelHost } from '@/chrome/panelRequests'
import { useGitLabConfigured, refreshBoard } from '@/gitlab/store'
import '@/styles/tokens.css'
import '@/styles/fonts.css'
import '@/styles/iconMasks.css'
import '@/icons/icon.css'

const w = window as any
w.actions = []; w.opened = []; w.copied = ''; w.accounts = []
Object.defineProperty(navigator, 'clipboard', { value: { writeText: async (text: string) => { w.copied = text } } })
const handlers = new Map<string, any>([['gitlab_request', () => ({ data: { revision: Date.now(), accounts: w.accounts, reviews: [], preferences: {} }, nextPage: null })]])
installFakeTauri('main', handlers, () => null)
const project = (id: string) => ({ id, name: id, tabs: [], roots: [], toolWindow: { open: false, height: 360, history: [], active: null } })
const bindings = [
  { key: 'ctrl+shift+m', command: 'sidebar.problems', when: null },
  { key: 'f4', command: 'sidebar.toggle', when: 'shellWindow' },
  { key: 'f4', command: 'problems.jumpToSource', when: 'problemsFocused' },
]
const boot: any = { role: { kind: 'shell', active: 'project', projects: ['project', 'second'] }, keymap: bindings, commands: [], workspace: {
  rev: 1, projects: { project: project('project'), second: project('second') },
  windows: { main: { kind: 'shell', active: 'project', projects: ['project', 'second'] } }, settings: { uiFontSize: 13, theme: 'light', git: {} },
} }
useWorkspace.setState({ boot })
const initial = { kind: 'ready', source: 'rust-analyzer', sources: [{ id: 'rustAnalyzer', label: 'rust-analyzer', status: { kind: 'ready' }, items: 3 }], items: [
  { path: 'src/main.rs', absPath: '/project/src/main.rs', line: 12, column: 5, severity: 'error', message: 'expected i32, found String', source: 'rust-analyzer', code: 'E0308' },
  { path: 'src/main.rs', absPath: '/project/src/main.rs', line: 3, column: 1, severity: 'warning', message: 'unused import', source: 'rust-analyzer' },
  { path: 'src/lib.rs', absPath: '/project/src/lib.rs', line: 7, column: 4, severity: 'warning', message: 'unused variable', source: 'rust-analyzer', stale: true },
] }
w.snapshot = initial
w.servers = [{ source: 'rustAnalyzer', label: 'rust-analyzer', status: { kind: 'ready' }, stopped: false, attached: false, version: '1.91.1', memoryBytes: 512 * 1024 ** 2 }]
languageServers.get = async () => structuredClone(w.servers)
languageServers.control = async (project, source, action) => {
  w.actions.push([project, source, action])
  if (w.holdControl) { w.holdControl = false; await new Promise(resolve => { w.finishControl = resolve }) }
  w.servers = w.servers.map((row: any) => ({ ...row, stopped: action === 'stop', status: action === 'stop' ? { kind: 'unavailable', reason: 'Stopped by user' } : { kind: 'ready' } }))
}
languageServers.logs = async (_, source, after) => ({ cursor: 1, truncated: false, entries: after ? [] : [{ sequence: 1, timestamp: 1, level: 'info', message: `${source}: Initialization completed` }] })
searchSessions.list = async () => []
git.repos = async () => []
git.worktrees = async () => []
branch.list = async () => []
clipboard.writeText = async text => { w.copied = text }
toolWindow.setLayout = async (project, patch) => {
  useWorkspace.setState(s => ({ boot: { ...s.boot, workspace: { ...s.boot!.workspace, projects: { ...s.boot!.workspace.projects,
    [project!]: { ...s.boot!.workspace.projects[project!]!, toolWindow: { ...s.boot!.workspace.projects[project!]!.toolWindow, ...patch } },
  } } } as any }))
  // Workspace events can reach React before the command acknowledgement.
  if (patch.open && w.layoutDelay) await new Promise(resolve => setTimeout(resolve, w.layoutDelay))
}
toolWindow.activate = async (project, active) => {
  useWorkspace.setState(s => ({ boot: { ...s.boot, workspace: { ...s.boot!.workspace, projects: { ...s.boot!.workspace.projects,
    [project!]: { ...s.boot!.workspace.projects[project!]!, toolWindow: { ...s.boot!.workspace.projects[project!]!.toolWindow, active } },
  } } } as any }))
}
registerPanelHost(() => {})
const run = createDispatcher({ fallback: () => {}, toggleSidebar: () => w.sidebarToggled = true })
installKeyGate({ bindings: () => bindings, context: () => ({ shellWindow: true, problemsFocused: useProblemsState.getState().focused }), run })
w.connect = async (enabled: boolean) => { w.accounts = enabled ? [{ id: 'account', username: 'user' }] : []; await refreshBoard() }
w.open = (project = 'project') => showProblems(project)
w.selection = useToolSelection
w.selectTool = (id: string) => selectTransientToolTab('project', id)
w.hide = () => toolWindow.setLayout('project', { open: false })
function Fixture() {
  const [snapshot, setSnapshot] = React.useState<any>(initial)
  const [file, setFile] = React.useState<string | null>('/project/src/main.rs')
  const [active, setActive] = React.useState('project')
  const [editorFocused, setEditorFocused] = React.useState(true)
  const [noise, setNoise] = React.useState(0)
  const problemsFocused = useProblemsState(s => s.focused)
  w.setEditorFocused = setEditorFocused; w.setNoise = setNoise
  React.useEffect(() => {
    if (!file) return
    const slot = claimStatusReadout(file, file.split('/').slice(-2))
    return () => slot.release()
  }, [file])
  const onOpenLocation = React.useCallback((...args: any[]) => { w.opened.push(args) }, [])
  const onRefresh = React.useCallback(() => { w.actions.push(['refresh']) }, [])
  w.setSnapshot = setSnapshot; w.setFile = setFile; w.setProject = (project: string) => {
    useWorkspace.setState(s => ({ boot: { ...s.boot, role: { ...s.boot!.role, active: project } } as any }))
    setActive(project)
  }
  const configured = useGitLabConfigured()
  const open = useWorkspace(s => s.boot?.workspace.projects[active]?.toolWindow.open)
  return <div data-noise={noise} data-problems-focused={problemsFocused} style={{ height: '100vh', display: 'flex', flexDirection: 'column', background: 'var(--panel)', color: 'var(--text)' }}>
    <div style={{ display: 'flex', flex: 1, minHeight: 0 }}><ActivityRail active="files" gitLabConfigured={configured} onSelect={() => {}}/><div style={{ flex: 1, display: 'flex', flexDirection: 'column', minWidth: 0 }}>
      <div style={{ flex: 1, padding: 20, fontFamily: 'var(--font-mono)' }}>src/main.rs<input aria-label="Editor"/></div>
      {open && <ToolWindowHost project={active} problems={{ snapshot, file, onOpenLocation, onRefresh }}/>}</div></div>
    <StatusBar editorFocused={editorFocused} diagnosticsControl={<DiagnosticsStatus project={active} projectName={active} snapshot={snapshot} onOpenProblems={() => run('sidebar.problems', null)}/>} />
  </div>
}
createRoot(document.getElementById('root')!).render(<Fixture/>); w.ready = true
