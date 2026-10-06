/** Exercise real creation actions and DOM focus; mock IPC and terminal hosting only. */
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'
import { JSDOM } from 'jsdom'

const ui = resolve(import.meta.dirname, '..')
mkdirSync(join(ui, 'node_modules/.cache'), { recursive: true })
const out = mkdtempSync(join(ui, 'node_modules/.cache/pane-focus-'))
const dom = new JSDOM('<!doctype html><div id="root"></div>', { url: 'https://cide.test?window=shell:test' })
globalThis.window = dom.window
globalThis.document = dom.window.document
globalThis.location = dom.window.location
const hosts = new Map()
globalThis.__paneFocusHosts = hosts
let frames = []
globalThis.requestAnimationFrame = (callback) => frames.push(callback)
const tick = () => new Promise((resolve) => setImmediate(resolve))
const frame = async () => {
  const ready = frames
  frames = []
  for (const callback of ready) callback(0)
  await tick()
}
let rev = 0, result, failure = null
const commands = []
window.__TAURI_INTERNALS__ = {
  metadata: { currentWindow: { label: 'shell:test' } },
  invoke: async (command, args) => {
    commands.push({ command, args })
    if (command === 'workspace_rev') return rev
    if (['pane_split', 'pane_add_row', 'tab_new_claude', 'tab_new_run'].includes(command)) {
      if (failure) throw failure
      return result
    }
  },
}

try {
  await build({
    configFile: false, root: ui, logLevel: 'error',
    resolve: { alias: [
      { find: '@/layout/paneHosts', replacement: '\0focus-hosts' },
      { find: '@/editor/languages', replacement: '\0focus-languages' },
      { find: '@', replacement: join(ui, 'src') },
    ] },
    plugins: [{
      name: 'focus-fixtures',
      resolveId(id) { if (id.startsWith('\0focus-')) return id },
      load(id) {
        if (id === '\0focus-hosts') return `
          export const peekHost = id => globalThis.__paneFocusHosts.get(id);
          export const destroyHost = id => globalThis.__paneFocusHosts.delete(id);
          export const noteLivePanes = () => {};
          export const releaseHost = () => {};
        `
        if (id === '\0focus-languages') return 'export const registerLanguages = () => [];'
      },
    }],
    build: { ssr: join(ui, 'src/store/workspace.ts'), outDir: out, emptyOutDir: false },
  })
  const { useWorkspace } = await import(pathToFileURL(join(out, 'workspace.js')).href)
  const role = { kind: 'shell', projects: ['project'], active: 'project' }
  const pane = (id, kind = 'claude') => ({ id, kind, role: 'auxiliary', session: null })
  const tab = (id, panes, focused) => ({
    id, kind: { kind: 'claudeFull', title: id, ephemeral: false },
    tree: { root: { kind: 'leaf', pane: focused }, panes, focused, maximized: null },
  })
  const initial = () => ({
    rev, settings: { theme: 'light' }, windows: { 'shell:test': role },
    projects: { project: {
      id: 'project', roots: [], detached: {}, activeTab: 'home', tabMru: ['home'],
      tabs: [tab('home', { old: pane('old') }, 'old')],
    } },
  })
  const writes = []
  const mount = (id) => {
    const el = document.createElement('div')
    const input = document.createElement('textarea')
    input.dataset.pane = id
    input.addEventListener('keydown', (event) => writes.push([id, event.key]))
    el.append(input)
    document.getElementById('root').append(el)
    hosts.set(id, { el, mounted: true, opened: true, terminal: { term: { focus: () => input.focus() } } })
    return input
  }
  const publish = (workspace) => useWorkspace.getState().applySnapshot(workspace)
  const cases = [
    ['harness row', 'addRow', { kind: 'newClaude' }],
    ['bash row', 'addRow', { kind: 'shell' }],
    ['harness split', 'splitPane', { kind: 'newClaude' }],
    ['bash split', 'splitPane', { kind: 'shell' }],
    ['new harness tab', 'newClaudeTab', { kind: 'newClaude' }],
    ['run tab', 'newRunTab', { kind: 'mirror', session: 'running-session' }],
  ]
  const start = async ([, action, intent]) => {
    hosts.clear()
    frames = []
    commands.length = 0
    writes.length = 0
    document.getElementById('root').replaceChildren()
    rev += 1
    const before = initial()
    useWorkspace.setState({ boot: { role, workspace: before }, mru: [], tabMru: {} })
    const old = mount('old')
    old.focus()
    const after = structuredClone(before)
    after.rev = ++rev
    const project = after.projects.project
    const fresh = pane('new', intent.kind === 'shell' ? 'shell' : 'claude')
    const asTab = action === 'newClaudeTab' || action === 'newRunTab'
    if (asTab) {
      project.tabs.push(tab('new-tab', { new: fresh }, 'new'))
      project.activeTab = 'new-tab'
    } else {
      project.tabs[0].tree.panes.new = fresh
      project.tabs[0].tree.focused = 'new'
    }
    result = action === 'newClaudeTab' ? 'new-tab' : { pane: 'new', intent }
    const expected = result
    const ws = useWorkspace.getState()
    const pending = action === 'addRow' ? ws.addRow('project', 'home', 'old', 'after', intent)
      : action === 'splitPane' ? ws.splitPane('project', 'home', 'old', 'row', 'after', intent)
      : action === 'newRunTab' ? ws.newRunTab('project', 'Run', intent)
      : ws.newClaudeTab('project')
    // The command has returned but its broadcast is still in flight: focus must wait.
    await tick()
    assert.equal(document.activeElement, old)
    assert.equal(frames.length, 0, 'creation waits for the workspace revision')
    publish(after)
    await tick()
    const input = mount('new')
    assert.equal(document.activeElement, old, 'selection alone does not focus before mount settles')
    return { after, old, input, pending, expected }
  }
  const finish = async (test) => {
    await frame()
    await frame()
    assert.deepEqual(await test.pending, test.expected, 'the action preserves its return value')
  }
  for (const spec of cases) {
    const test = await start(spec)
    await finish(test)
    assert.equal(document.activeElement, test.input, `${spec[0]} puts the keyboard in the new pane`)
    document.activeElement.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'x', bubbles: true }))
    assert.deepEqual(writes, [['new', 'x']], 'typing reaches the newly created terminal')
    test.old.focus()
    const update = structuredClone(test.after)
    update.rev = ++rev
    publish(update)
    await frame()
    await frame()
    assert.equal(document.activeElement, test.old, 'an unrelated workspace update does not claim focus')
  }

  for (const scenario of ['pane switch', 'tab switch', 'project switch', 'closed', 'parked', 'unopened', 'hidden', 'hidden-check', 'detached']) {
    const test = await start(cases[0])
    await frame()
    const update = structuredClone(test.after)
    const project = update.projects.project
    if (scenario === 'pane switch') project.tabs[0].tree.focused = 'old'
    if (scenario === 'tab switch') project.activeTab = 'another-tab'
    if (scenario === 'project switch') update.windows['shell:test'].active = 'another-project'
    if (scenario === 'closed') delete project.tabs[0].tree.panes.new
    if (scenario === 'parked') { hosts.get('new').mounted = false; hosts.get('new').el.remove() }
    if (scenario === 'unopened') hosts.get('new').opened = false
    if (scenario === 'hidden') hosts.get('new').el.style.visibility = 'hidden'
    if (scenario === 'hidden-check') hosts.get('new').el.checkVisibility = () => false
    if (scenario === 'detached') update.windows['shell:test'] = { kind: 'detachedPane', project: 'project', tab: 'home', pane: 'old' }
    update.rev = ++rev
    publish(update)
    await frame()
    await test.pending
    assert.equal(document.activeElement, test.old, `${scenario} cancels the stale focus transfer`)
  }
  // A failed creation never schedules a keyboard transfer.
  failure = new Error('spawn refused')
  await assert.rejects(useWorkspace.getState().addRow('project', 'home'), /spawn refused/)
  assert.equal(frames.length, 0)
  console.log('pane focus: creation, keyboard input, synchronization and stale/hidden pane guards pass')
} finally {
  delete globalThis.__paneFocusHosts
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
