// Browser-audit fixture. Real modal, bottom host, dispatcher and key gate; deterministic IPC.
import React from 'react'
import { createRoot } from 'react-dom/client'
import { installFakeTauri } from '@/demo/fakeTauri'
import { EditorView } from '@codemirror/view'
import { focusedCaret, claimCaret } from '@/editor/caretTrack'
import { registerReveal, requestReveal, pendingReveals } from '@/editor/revealRequest'
import { deriveContext } from '@/keys/context'
import { GoToLine } from '@/overlays/GoToLine'
import { SearchEverywhere } from '@/search/SearchEverywhere'
import { ToolWindowHost } from '@/toolwindow/ToolWindowHost'
import { useSearchSessions, syncSessions } from '@/search/store'
import { useOverlays, closeOverlay } from '@/overlays/store'
import { useWorkspace } from '@/store/workspace'
import { useToolSelection } from '@/toolwindow/selection'
import { searchSessions, file, toolWindow, git, branch, diagnostics, clipboard } from '@/ipc/client'
import { buildKeymap } from '@/keys/keymap'
import { createDispatcher } from '@/keys/dispatch'
import { installKeyGate } from '@/keys/gate'
import '@/styles/fonts.css'
import '@/styles/tokens.css'
import '@/styles/iconMasks.css'
import '@/icons/icon.css'
import '@/editor/highlight.css'

installFakeTauri('main', new Map(), () => null)
const w = window as any
w.docEvents = []; w.definitions = []; w.scopeQueries = [];
w.caret = focusedCaret; w.pendingReveals = pendingReveals; w.requestReveal = requestReveal;
const originalCaret = claimCaret('/original.ts'); originalCaret.set(9, 3, 80);
registerReveal('/original.ts', () => {});
w.previewView = () => EditorView.findFromDOM(document.querySelector('[data-cide-embedded-editor]')!);
w.keyContext = () => deriveContext(useWorkspace.getState().boot);
w.opened = []; w.ran = []; w.pages = []; w.closedSearches = []
const boot: any = { role: { kind: 'shell', active: 'project', projects: ['project'] }, workspace: {
  rev: 1, projects: { project: { id: 'project', tabs: [], activeTab: null, roots: [], toolWindow: { open: false, height: 320, history: [], active: null } } },
  windows: { main: { kind: 'shell', active: 'project', projects: ['project'] } },
  settings: { uiFontSize: 13, theme: 'light', git: {} },
} }
useWorkspace.setState({ boot })
clipboard.writeText = async text => { w.clipboard = text }
diagnostics.didOpen = async (_, path) => { w.docEvents.push(['open', path]) }
diagnostics.didClose = async (_, path) => { w.docEvents.push(['close', path]) }
diagnostics.definition = async (_, path, line, column) => { w.definitions.push([path, line, column]); return { kind: 'found', path: '/definition.ts', line: 7, column: 1, interfaceMethod: false } }
searchSessions.suggestScopes = async (_, input) => {
  w.scopeQueries.push(input)
  if (input === 'slow') await new Promise(r => setTimeout(r, 300))
  return ['game', 'game/ui', 'addons/game'].filter(path => path.includes(input))
}
const jobs = new Map<string, any>()
let serial = 0
const delay = (ms: number) => new Promise(r => setTimeout(r, ms))
searchSessions.start = async (project, descriptor) => {
  if (descriptor.query.pattern === 'delayed') await delay(250)
  for (const old of jobs.values()) if (!old.pinned) jobs.delete(old.id)
  const session = { id: `s${++serial}`, project, descriptor: structuredClone(descriptor), pinned: false, generation: 0 }
  jobs.set(session.id, session)
  return structuredClone(session)
}
searchSessions.pin = async (_, id) => {
  if (w.holdPin) await new Promise(resolve => { w.finishPin = resolve })
  const job = jobs.get(id); if (!job) throw new Error('Search closed')
  job.pinned = true
  return structuredClone(job)
}
searchSessions.list = async () => {
  const snapshot = [...jobs.values()].filter(s => s.pinned).map(s => structuredClone(s))
  if (w.holdList) { w.holdList = false; await new Promise(resolve => { w.finishList = resolve }) }
  return snapshot
}
searchSessions.cancel = async (_, id) => { if (!jobs.get(id)?.pinned) jobs.delete(id) }
searchSessions.close = async (_, id) => { jobs.delete(id); w.closedSearches.push(id) }
searchSessions.rerun = async (_, id) => { const job = jobs.get(id); job.generation++; return structuredClone(job) }
searchSessions.page = async (_, id, offset = 0) => {
  const session = structuredClone(jobs.get(id))
  if (!session) throw new Error('Search closed')
  w.pages.push([id, offset])
  const { source, query } = session.descriptor
  const q = query.pattern
  const empty = q === 'absent'
  const bad = query.mode === 'regex' && q === '['
  const total = empty || bad ? 0 : 450
  const path = q === 'unavailable' ? '/missing.ts' : `/${q || 'alpha'}.ts`
  const hits = Array.from({ length: Math.max(0, Math.min(200, total - offset)) }, (_, index) => {
    const hitPath = q === 'groups' && offset + index >= 230 ? '/other.ts' : path
    return { path: hitPath, rel: hitPath.slice(1), line: offset + index + 1,
      text: `\t  const ${q || 'alpha'} = ${index};`, start: 9, end: 9 + (q || 'alpha').length }
  })
  return { session, running: false, errors: [],
    files: source === 'all' || source === 'files' ? { items: empty ? [] : ['alpha', 'beta'].map(name => ({ value: `/${name}.ts`, text: `${name}.ts`, indices: [], source: null })), query: q, matched: empty ? 0 : 2, total: 2, running: false } : null,
    symbols: source === 'all' || source === 'symbols' ? { items: empty ? [] : [{ path, rel: path.slice(1), name: q || 'alpha', line: 1, column: 7, endColumn: 12, kind: 'function', indices: [] }], query: q, matched: empty ? 0 : 1, total: 1, running: false, truncated: false } : null,
    text: source === 'all' || source === 'text' ? { query, hits, offset, total, running: false, truncated: false, error: bad ? { kind: 'pattern', detail: 'Invalid regular expression' } : null } : null,
  } as any
}
file.read = async path => {
  if (path === '/missing.ts') throw { kind: 'io', detail: 'The file was removed.' }
  return { path, text: Array.from({ length: 500 }, (_, i) => `\t  const ${path.slice(1, -3)} = ${i};`).join('\n') } as any
}
file.open = async (_, path) => { w.opened.push(['file', path]); return null as any }
git.repos = async () => []
branch.list = async () => []
toolWindow.setLayout = async (_, patch) => {
  const before = useWorkspace.getState().boot as any
  useWorkspace.setState({ boot: { ...before, workspace: { ...before.workspace, rev: before.workspace.rev + 1,
    projects: { project: { ...before.workspace.projects.project, toolWindow: { ...before.workspace.projects.project.toolWindow, ...patch } } } } } })
}
const bindings = [
  { key: 'ctrl+g', command: 'navigate.line', when: 'editorFocused' },
  { key: 'ctrl+b', command: 'navigate.definition', when: 'editorFocused' },
  { key: 'shift shift', command: 'search.everywhere', when: null },
  { key: 'ctrl+p', command: 'picker.files', when: null },
  { key: 'ctrl+shift+p', command: 'palette.commands', when: null },
  { key: 'ctrl+shift+f', command: 'sidebar.search', when: null },
  { key: 'ctrl+alt+shift+n', command: 'picker.symbols', when: null },
]
const commands = [{ id: 'audit.action', title: 'Audit action', group: 'Test', keywords: [], when: null, unavailable: null }]
boot.commands = commands
const run = createDispatcher({ fallback: id => w.ran.push(id), toggleSidebar: () => {} })
installKeyGate({ bindings: () => bindings, context: () => ({ ...deriveContext(useWorkspace.getState().boot), overlayOpen: useOverlays.getState().open !== null }), run })
const actions: any = {
  openFile: path => w.opened.push(['file', path]), openFileInSplit: path => w.opened.push(['split', path]), mentionFile: path => w.opened.push(['mention', path]),
  goToSymbol: (...args) => w.opened.push(['location', ...args]), runCommand: id => w.ran.push(id), runCommandInNewSession: id => w.ran.push(`session:${id}`),
}
w.run = run; w.layout = (searchModalLayout: string) => useWorkspace.setState(s => ({ boot: { ...s.boot, workspace: { ...s.boot!.workspace, settings: { ...s.boot!.workspace.settings, searchModalLayout } } } as any }));
w.search = useSearchSessions; w.jobs = jobs; w.sync = () => syncSessions('project'); w.closeSearch = closeOverlay
w.toolSelection = useToolSelection; w.panel = (open: boolean) => toolWindow.setLayout('project', { open })
w.searchUiSize = (size: number) => useWorkspace.setState(s => ({ boot: { ...s.boot, workspace: { ...s.boot!.workspace, settings: { ...s.boot!.workspace.settings, uiFontSize: size } } } as any }))
function Fixture() {
  const overlay = useOverlays(s => s.open)
  const open = useWorkspace(s => s.boot?.workspace.projects.project?.toolWindow.open)
  return <><input id="behind" aria-label="Background editor"/><button id="open" onClick={() => run('search.everywhere', null)}>Search</button>
    {open && <div style={{ position: 'fixed', bottom: 0, width: '100%', height: 350 }}><ToolWindowHost project="project"/></div>}
    {overlay === 'goto' && <GoToLine onDismiss={closeOverlay} onGoTo={(path, line, column) => requestReveal(path, { line, column, endColumn: column, focus: true })} />}
    {overlay === 'search' && <SearchEverywhere project="project" commands={commands} keymap={buildKeymap(bindings)} context={{}} actions={actions} onDismiss={closeOverlay}/>}
  </>
}
createRoot(document.getElementById('root')!).render(<Fixture/>); w.ready = true
