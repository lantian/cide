/** Real hosts/stores against a controlled IPC transport; no OpenSpec binary or repository writes. */
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { resolve } from 'node:path'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

const UI = resolve(import.meta.dirname, '..')
mkdirSync(resolve(UI, 'node_modules/.cache'), { recursive: true })
const out = mkdtempSync(resolve(UI, 'node_modules/.cache/openspec-performance-'))
const dom = new JSDOM('<!doctype html><div id="root"></div>', { url: 'https://cide.test', pretendToBeVisual: true })
for (const name of ['window', 'document', 'location', 'HTMLElement', 'Element', 'Node', 'MutationObserver', 'DOMParser', 'HTMLDetailsElement']) globalThis[name] = dom.window[name]
globalThis.self = dom.window
globalThis.IS_REACT_ACT_ENVIRONMENT = true
globalThis.requestAnimationFrame = (callback) => setTimeout(callback, 0)
globalThis.cancelAnimationFrame = clearTimeout
globalThis.ResizeObserver = dom.window.ResizeObserver = class { observe() {} disconnect() {} unobserve() {} }
Object.defineProperties(dom.window.HTMLElement.prototype, {
  offsetHeight: { get() { return this.dataset.audit === 'virtualList' ? 500 : 40 } },
  offsetWidth: { get() { return 320 } },
})
dom.window.HTMLElement.prototype.scrollIntoView = function () {}
dom.window.HTMLCanvasElement.prototype.getContext = () => null
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no }); return { promise, resolve, reject } }
const calls = []
const callbacks = new Map()
const listeners = new Map()
let nextCallback = 1
let respond = async () => undefined
window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} }
window.__TAURI_INTERNALS__ = {
  transformCallback: (callback) => { const id = nextCallback++; callbacks.set(id, callback); return id },
  unregisterCallback: (id) => callbacks.delete(id),
  invoke: async (command, args) => {
    if (command === 'plugin:event|listen') { listeners.set(args.event, args.handler); return args.handler }
    if (command === 'plugin:event|unlisten') { listeners.delete(args.event); return }
    calls.push({ command, args })
    return respond(command, args)
  },
}
const board = { kind: 'ready', root: '/repo', changes: [], specs: [], commands: [] }
const snapshot = { revision: 1, board }
const change = (name = 'one') => ({ name, title: name, deltas: [], artifacts: [], progress: { completed: 1, total: 1, tasks: [] }, validation: { valid: true, issues: [] }, origin: { kind: 'active' } })
const preview = { archiveRoot: '/repo', change: 'one', branch: null, refusals: [], specsTouched: [] }
let root
try {
  execFileSync('node', ['node_modules/vite/bin/vite.js', 'build', '--ssr', 'src/sidebar/OpenSpecPanel/PerformanceSmokeEntry.tsx', '--outDir', out, '--logLevel', 'error'], { cwd: UI, stdio: 'inherit' })
  const M = await import(`file://${resolve(out, 'PerformanceSmokeEntry.js')}`)
  const { createRoot } = await import('react-dom/client')
  root = createRoot(document.getElementById('root'))
  const render = async (node) => { await act(async () => { root.render(node) }) }
  const flush = () => new Promise((resolve) => setImmediate(resolve))
  const reset = () => { M.retainSpecProjects([]); M.useSpecData.setState({ epochs: {} }); calls.length = 0 }

  // Concurrent consumers, generation changes, independent changes and worktree artifact roots.
  reset()
  let first = deferred(), reads = 0
  respond = (command) => command === 'spec_change' ? (++reads === 1 ? first.promise : change()) : { text: 'proposal', truncated: false }
  const a = M.readChange('p', '/repo', 'one'), b = M.readChange('p', '/repo', 'one')
  assert.equal(reads, 1)
  M.invalidateSpecData('p', snapshot, { full: false, paths: ['openspec/changes/one/proposal.md'] })
  first.resolve({ ...change(), title: 'stale' })
  assert.equal((await a).title, 'one'); assert.equal((await b).title, 'one'); assert.equal(reads, 2)
  M.invalidateSpecData('p', snapshot, { full: false, paths: ['openspec/changes/two/tasks.md'] })
  await M.readChange('p', '/repo', 'one'); assert.equal(reads, 2)
  await M.readArtifact('p', '/repo/.cide/worktrees/spec-one/openspec/changes/one/proposal.md', '/repo/.cide/worktrees/spec-one')
  assert.equal(calls.at(-1).args.worktree, '/repo/.cide/worktrees/spec-one')
  for (let i = 0; i < 70; i++) await M.readChange('p', '/repo', `change-${i}`)
  const beforeEvicted = reads
  await M.readChange('p', '/repo', 'one'); assert.equal(reads, beforeEvicted + 1, 'least recently used entries are evicted')

  // Summary revisions reject late responses, and a single app listener owns spec invalidation.
  M.useSpec.setState({ project: 'p', board, revision: 4 })
  M.useSpec.getState().adopt('p', { revision: 3, board: { kind: 'absent', path: '/wrong', hint: 'old' } })
  assert.equal(M.useSpec.getState().board.kind, 'ready')
  const appSource = readFileSync(resolve(UI, 'src/App.tsx'), 'utf8')
  const hosts = ['OpenSpecPanel/OpenSpecPanelHost.tsx', 'OpenSpecPanel/SpecTab.tsx', 'TasksPanel/TasksPanelHost.tsx', 'TasksPanel/TaskDetailHost.tsx']
  assert.equal((appSource.match(/specEvents\.onChanged/g) ?? []).length, 1)
  for (const host of hosts) assert.ok(!readFileSync(resolve(UI, 'src/sidebar', host), 'utf8').includes('followSpecRuns('))
  reset()
  let checkout = deferred()
  respond = (command) => command === 'spec_checkouts' ? checkout.promise : []
  const stop = M.followSpecRuns('p')
  await flush()
  assert.equal(calls.filter((c) => c.command === 'spec_checkouts').length, 1, 'attach only reads once')
  assert.equal(listeners.has('cide://spec-changed'), false, 'run follower does not duplicate the app spec listener')
  const refresh = M.useSpecRuns.getState().refreshCheckouts()
  checkout.resolve([]); await refresh
  assert.equal(calls.filter((c) => c.command === 'spec_checkouts').length, 2, 'an invalidation while pending queues exactly one follow-up')
  stop(); await flush(); assert.equal(listeners.size, 0)

  // Hidden tabs mount without a read, preserve content while hidden, then catch up once active.
  reset()
  respond = async (command, args) => command === 'spec_change' ? change(args.change) : command === 'spec_artifact' ? { text: '# Capability', truncated: false } : undefined
  M.useSpec.setState({ project: 'p', board, revision: 1 })
  const tab = (active) => React.createElement(M.SpecTab, { project: 'p', change: 'one', active })
  await render(tab(false)); assert.equal(calls.filter((c) => c.command === 'spec_change').length, 0)
  await render(tab(true)); assert.equal(calls.filter((c) => c.command === 'spec_change').length, 1)
  await render(tab(false))
  await act(async () => M.invalidateSpecData('p', snapshot, { full: false, paths: ['openspec/changes/one/proposal.md'] }))
  assert.equal(calls.filter((c) => c.command === 'spec_change').length, 1)
  await render(tab(true)); assert.equal(calls.filter((c) => c.command === 'spec_change').length, 2)
  await render(null)
  calls.length = 0
  const capability = (active) => React.createElement(M.SpecTab, { project: 'p', spec: 'cap', active })
  await render(capability(false)); assert.equal(calls.length, 0)
  await render(capability(true)); assert.equal(calls.filter((c) => c.command === 'spec_artifact').length, 1)
  await render(null)

  // A linked proposal can be approved on the page; launch failures remain retryable.
  reset()
  M.clearNotices()
  const opened = []
  M.useWorkspace.setState({ boot: { role: { kind: 'shell', active: 'p' } }, newRunTab: async (...args) => opened.push(args) })
  const tracked = { id: 't-1', agent: null, session: null, status: 'todo', change: 'one' }
  const taskBoard = (status) => M.useTasks.setState({ project: 'p', board: { kind: 'ready', rev: 1, tasks: [{ ...tracked, status }] }, selected: null })
  taskBoard('todo')
  M.useSpec.setState({ project: 'p', board, revision: 1 })
  M.useSpecRuns.setState({ project: 'p', runs: [], checkouts: [] })
  let launch = deferred(), applied = false, completed = 0, invalid = false, archived = false, integrateSuccess = false
  const checkoutRow = { change: 'one', branch: 'cide/spec-one', completedTasks: completed, totalTasks: 1, dirty: false, unmerged: 1 }
  respond = (command, args) => {
    if (command === 'spec_change') return {
      ...change(), origin: archived ? { kind: 'archived', folder: 'stamp-one' } : { kind: 'active' },
      progress: { completed: args.worktree ? completed : 0, total: invalid && !applied ? 0 : 1, tasks: [] },
      validation: { valid: !invalid || invalid === 'blocking', issues: invalid ? [{ level: 'ERROR', path: '', message: 'Invalid change' }] : [] },
    }
    if (command === 'project_harness_effective') return { consoleHarness: 'codex' }
    if (command === 'spec_session_start') return launch.promise
    if (command === 'spec_runs') return [{ run: 'apply-1', op: 'apply', change: 'one', label: 'OpenSpec · Codex', state: { state: 'running' } }]
    if (command === 'agents_run_open') return { kind: 'live', session: 'session-1', continues: null }
    if (command === 'spec_checkouts') return applied ? [{ ...checkoutRow, completedTasks: completed }] : []
    if (command === 'spec_integrate') {
      if (!integrateSuccess) return { kind: 'refused', reason: 'Verify failed' }
      M.invalidateSpecData('p', snapshot, { full: true, paths: [] })
      return { kind: 'merged', files: 1, kept: [] }
    }
    if (command === 'spec_publish') return { kind: 'refused', reason: 'No remote configured' }
    if (command === 'spec_change_plan') return preview
    return []
  }
  await render(tab(true))
  const approve = () => document.querySelector('[data-audit="specTabApprove"]')
  assert.equal(approve().textContent, 'Approve')
  await act(async () => { approve().click(); approve().click(); for (let i = 0; i < 8; i++) await flush() })
  assert.equal(calls.filter((c) => c.command === 'spec_session_start').length, 1)
  assert.equal(approve().disabled, true)
  await act(async () => { launch.reject(new Error('Apply could not start')); await flush() })
  assert.equal(approve().disabled, false, 'failed approval can be retried')
  assert.ok(M.noticeSnapshot().some((notice) => notice.text.includes('Apply could not start')))
  launch = deferred()
  await act(async () => { approve().click(); for (let i = 0; i < 8; i++) await flush() })
  assert.equal(calls.filter((c) => c.command === 'spec_session_start').length, 2)
  assert.deepEqual(calls.filter((c) => c.command === 'spec_session_start').at(-1).args.request,
    { op: 'apply', change: 'one', launcher: { kind: 'harness', harness: 'codex' } })
  await act(async () => { launch.resolve('apply-1'); for (let i = 0; i < 8; i++) await flush() })
  assert.equal(opened.length, 1, 'approval presents the Apply session')
  assert.equal(approve(), null, 'a live Apply suppresses approval before the task status event arrives')

  // The application receives the creation invalidation, discovers the worktree, then follows
  // its checklist instead of the still-unticked root proposal, all without remounting the tab.
  await act(async () => M.useSpecRuns.getState().refreshCheckouts())
  assert.equal(M.useSpecRuns.getState().checkouts.length, 0)
  applied = true
  await act(async () => {
    taskBoard('doing')
    M.invalidateSpecData('p', snapshot, { full: true, paths: [] })
    await M.useSpecRuns.getState().refreshCheckouts()
  })
  assert.equal(approve(), null)
  assert.equal(calls.filter((c) => c.command === 'spec_change').at(-1).args.worktree, '/repo/.cide/worktrees/spec-one')
  const readyButtons = () => [...document.querySelectorAll('[data-audit="specTabReadyAct"]')].map((node) => node.dataset.act)
  assert.deepEqual(readyButtons(), [])
  completed = 1
  await act(async () => {
    taskBoard('done')
    M.invalidateSpecData('p', snapshot, { full: false, paths: ['.cide/worktrees/spec-one/openspec/changes/one/tasks.md'] })
    await M.useSpecRuns.getState().refreshCheckouts()
  })
  assert.deepEqual(readyButtons(), ['publish', 'integrate', 'archive'])
  assert.equal(document.querySelector('[data-audit="specTabStart"]').textContent, 'Open t-1')
  await act(async () => { document.querySelector('[data-act="integrate"]').click(); await flush() })
  assert.equal(calls.filter((c) => c.command === 'spec_integrate').length, 1)
  assert.ok(M.noticeSnapshot().some((notice) => notice.detail === 'Verify failed'))
  await act(async () => { document.querySelector('[data-act="publish"]').click(); await flush() })
  assert.equal(calls.filter((c) => c.command === 'spec_publish').length, 1)
  assert.ok(M.noticeSnapshot().some((notice) => notice.detail === 'No remote configured'))
  await act(async () => { document.querySelector('[data-act="archive"]').click(); await flush() })
  assert.equal(M.useSpecConfirm.getState().confirming.phase, 'ready', 'the page reuses Archive confirmation')
  M.useSpecConfirm.getState().set(null)
  invalid = true
  await act(async () => M.invalidateSpecData('p', snapshot, { full: true, paths: [] }))
  assert.deepEqual(readyButtons(), [], 'invalid completed changes do not offer completion actions')
  invalid = 'blocking'
  await act(async () => M.invalidateSpecData('p', snapshot, { full: true, paths: [] }))
  assert.deepEqual(readyButtons(), [], 'blocking issues prevent completion even if the valid flag is true')
  invalid = false
  archived = true
  await act(async () => M.invalidateSpecData('p', snapshot, { full: true, paths: [] }))
  assert.deepEqual(readyButtons(), ['publish', 'integrate'], 'archiving does not imply the checkout was merged')
  integrateSuccess = true
  await act(async () => { document.querySelector('[data-act="integrate"]').click(); for (let i = 0; i < 8; i++) await flush() })
  assert.ok(readyButtons().includes('integrate'), 'a checkout kept for a live session still exists')
  // Retirement happens asynchronously after the integration response and publishes its own
  // full invalidation; the application refreshes the checkout list on that event.
  applied = false
  await act(async () => {
    M.invalidateSpecData('p', snapshot, { full: true, paths: [] })
    await M.useSpecRuns.getState().refreshCheckouts()
  })
  assert.equal(M.useSpecRuns.getState().checkouts.length, 0)
  assert.deepEqual(readyButtons(), [], 'Integrate disappears immediately once its worktree is removed')
  await render(null)

  // The screenshot's case: an early Apply ended with an incomplete 0/0 change and no task.
  // Keeping its idle child alive must not hide the Apply dialog for a deliberate retry.
  completed = 0
  invalid = true
  archived = false
  M.useTasks.setState({ board: { kind: 'ready', rev: 1, tasks: [] } })
  M.useSpecRuns.setState({ runs: [{ run: 'apply-1', op: 'apply', change: 'one', text: '', label: 'OpenSpec · Codex', state: { state: 'idle' }, turnComplete: true }] })
  M.invalidateSpecData('p', snapshot, { full: true, paths: [] })
  await render(tab(true))
  assert.equal(document.querySelector('[data-audit="specTabStart"]').textContent, 'Apply…')
  assert.ok(document.querySelector('[data-audit="specTabOpenSession"]'))
  await act(async () => document.querySelector('[data-audit="specTabStart"]').click())
  assert.equal(M.useApplyDialog.getState().change, 'one', 'Apply opens the existing launcher dialog')
  M.useApplyDialog.getState().close()
  await render(null)
  M.useTasks.setState({ project: null, board: { kind: 'unknown' } })
  M.useSpecRuns.setState({ project: null, runs: [], checkouts: [] })
  M.useWorkspace.setState({ boot: null })

  respond = async () => { throw new Error('board read failed') }
  await M.useSpec.getState().refresh(true)
  assert.equal(M.useSpec.getState().board.kind, 'unusable')
  respond = async () => ({ revision: 5, board })
  await M.useSpec.getState().refresh(true)
  assert.equal(M.useSpec.getState().board.kind, 'ready')

  // 5,000 changes plus 5,000 specs: only the viewport and overscan mount. Last rows remain reachable.
  const large = { ...board, changes: Array.from({ length: 5000 }, (_, i) => ({ name: `change-${String(i).padStart(4, '0')}`, completed: 1, total: 2, status: '' })), specs: Array.from({ length: 5000 }, (_, i) => ({ id: `spec-${String(i).padStart(4, '0')}`, requirements: 2 })) }
  const started = performance.now()
  await render(React.createElement(M.OpenSpecPanelView, { ...M.SPEC_STORIES.board, board: large }))
  const rows = document.querySelectorAll('[data-audit="openspecChangeRow"], [data-audit="openspecSpecRow"]')
  assert.ok(rows.length > 0 && rows.length < 100, `bounded rows: ${rows.length}`)
  console.log(`openspec large fixture: ${rows.length} mounted rows / 10,000 items; ${Math.round(performance.now() - started)}ms in jsdom`)
  const scroll = document.querySelector('[data-audit="virtualList"]')
  await act(async () => { scroll.scrollTop = 5000 * 118 + 5000 * 30 - 500; scroll.dispatchEvent(new window.Event('scroll')) })
  assert.ok(document.querySelector('[data-spec="spec-4999"]'), 'last spec is reachable by scrolling')
  await render(null)

  // Preview errors, double clicks, root pinning, conflicts, failure after dismissal and retry.
  reset()
  let planRead = deferred(), archive = deferred()
  respond = (command) => command === 'spec_change_plan' ? planRead.promise : command === 'spec_change_archive' ? archive.promise : []
  M.archiveChange('p', 'one'); M.archiveChange('p', 'one')
  assert.equal(M.useSpecConfirm.getState().confirming.phase, 'preparing')
  assert.equal(calls.length, 1)
  planRead.resolve(preview); await planRead.promise; await flush()
  assert.equal(M.useSpecConfirm.getState().confirming.phase, 'ready')
  const executing = M.confirmArchive(); await M.confirmArchive()
  assert.equal(calls.filter((c) => c.command === 'spec_change_archive').length, 1)
  assert.equal(calls.at(-1).args.expectedRoot, '/repo')
  assert.equal(M.useSpecConfirm.getState().confirming.phase, 'archiving')
  M.useSpecConfirm.getState().set(null)
  M.archiveChange('p', 'one'); assert.equal(M.useSpecConfirm.getState().confirming.phase, 'archiving')
  archive.resolve({ kind: 'conflicts', paths: ['spec.md'] }); await executing
  assert.equal(M.useSpecConfirm.getState().confirming.phase, 'failed')
  assert.match(M.useSpecConfirm.getState().confirming.error, /spec.md/)
  planRead = deferred(); await M.confirmArchive()
  assert.equal(M.useSpecConfirm.getState().confirming.phase, 'preparing')
  assert.equal(calls.at(-1).args.force, true)
  planRead.reject(new Error('CLI unavailable')); await planRead.promise.catch(() => {}); await flush()
  assert.equal(M.useSpecConfirm.getState().confirming.phase, 'failed')

  // Thousands of archive paths share the same measured-list bound.
  M.useSpecConfirm.getState().set({ project: 'p', change: 'one', phase: 'ready', error: null, plan: { ...preview, specsTouched: Array.from({ length: 5000 }, (_, i) => ({ spec: `cap-${i}`, requirements: 2, operation: 'added' })) } })
  await render(React.createElement(M.SpecActsConfirm))
  assert.ok(document.querySelectorAll('[data-index]').length < 100)
  await render(null)
  M.useSpecConfirm.getState().set(null)
  // A confirmed success after dismissal stays closed; a JSON refusal remains actionable.
  archive = deferred()
  respond = (command) => command === 'spec_change_plan' ? preview : command === 'spec_change_archive' ? archive.promise : []
  M.archiveChange('p', 'one'); await flush()
  const dismissed = M.confirmArchive()
  M.useSpecConfirm.getState().set(null)
  archive.resolve({ kind: 'accepted', archive: { root: '/repo', archivedAs: 'stamp-one', path: '/repo/openspec/changes/archive/stamp-one' } })
  await dismissed
  assert.equal(M.useSpecConfirm.getState().confirming, null)
  respond = (command) => command === 'spec_change_plan' ? preview : { kind: 'refused', plan: { ...preview, refusals: ['Task checklist is incomplete'] } }
  M.archiveChange('p', 'one'); await flush(); await M.confirmArchive()
  assert.equal(M.useSpecConfirm.getState().confirming.phase, 'failed')
  assert.match(M.useSpecConfirm.getState().confirming.error, /incomplete/)
  M.useSpecConfirm.getState().set(null)
  console.log('openspec performance: ok (shared reads, invalidation, hidden tabs, linked-task approval and completion, bounded lists, archive state and retries)')
} finally {
  if (root) await act(async () => root.unmount())
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
