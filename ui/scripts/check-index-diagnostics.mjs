import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { resolve } from 'node:path'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync('node_modules/.cache/cide-index-diagnostics-')
const dom = new JSDOM('<!doctype html><div id="root"></div>', { url: 'https://cide.test', pretendToBeVisual: true })
for (const name of ['window', 'document', 'location', 'HTMLElement', 'Element', 'Node', 'MutationObserver']) globalThis[name] = dom.window[name]
Object.defineProperty(globalThis, 'navigator', { configurable: true, value: dom.window.navigator })
globalThis.self = dom.window
globalThis.IS_REACT_ACT_ENVIRONMENT = true
globalThis.requestAnimationFrame = (callback) => setTimeout(callback, 0)
globalThis.cancelAnimationFrame = clearTimeout
const { createRoot } = await import('react-dom/client')
const calls = []
let copied = ''
let copyFails = false
Object.defineProperty(navigator, 'clipboard', { value: { writeText: async (text) => {
  if (copyFails) throw new Error('clipboard unavailable')
  copied = text
} } })
const usage = { projectBytes: 512 * 1048576, projectLimitBytes: 512 * 1048576, totalBytes: 900 * 1048576, totalLimitBytes: 2048 * 1048576 }
const report = {
  generation: 1, indexing: false, files: 100, dirs: 5, roots: ['/project'],
  showHiddenFiles: true, showIgnoredFiles: true, usage,
  indexExcludedFolders: ['.cide/worktrees', '.cide/tasks'], exclusionsSource: 'global', browseCacheBytes: 0,
  refusal: { reason: 'projectBudget', allocation: 'files', path: '/project/ui/node_modules/x.js', folder: '/project/ui/node_modules', requestedBytes: 1234, usage },
  folders: [{ root: '/project', path: '/project/ui/node_modules', relativePath: 'ui/node_modules', files: 80, dirs: 3, bytes: 400 * 1048576 }],
}
let reply = async () => structuredClone(report)
let restartReply = async () => { throw new Error('unexpected restart') }
window.__TAURI_INTERNALS__ = {
  transformCallback: () => 1,
  invoke: async (command, args) => {
    calls.push({ command, args })
    if (command === 'fs_index_diagnostics') return reply()
    if (command === 'fs_restart_index') return restartReply()
    if (command === 'settings_get') return { consoleHarness: 'claude' }
    return null
  },
}
let root
try {
  execFileSync('node', ['node_modules/vite/bin/vite.js', 'build', '--ssr', 'src/sidebar/IndexWarning.tsx', '--outDir', out, '--logLevel', 'error'], { stdio: 'inherit' })
  const { IndexWarning } = await import(`file://${resolve(out, 'IndexWarning.js')}`)
  root = createRoot(document.getElementById('root'))
  const status = { generation: 1, indexLimit: 'projectBudget', indexing: false, files: 100, dirs: 5, indexBytes: usage.projectBytes, rows: 5, watch: { backend: 'native', watchedDirs: 5, reason: null } }
  const render = async (project = 'p', changes = {}) => act(async () => root.render(React.createElement(IndexWarning, { key: project, project, status: { ...status, ...changes } })))
  const button = (label, scope = document) => [...scope.querySelectorAll('button')].find((b) => b.textContent === label)
  const click = async (label, scope) => {
    const target = button(label, scope)
    assert.ok(target, `button exists: ${label}`)
    await act(async () => { target.focus(); target.click() })
  }
  const dialog = () => document.querySelector('[data-audit="indexDiagnostics"]')
  await render()
  await click('Settings')
  assert.deepEqual(calls.find((c) => c.command === 'tab_open_settings')?.args, { project: 'p', section: 'files' }, 'Settings opens the project tab directly on Files')
  await click('Details')
  assert.ok(dialog())
  assert.equal(document.activeElement.textContent, 'Close')
  assert.match(dialog().textContent, /ui\/node_modules/)
  assert.match(dialog().textContent, /Parent totals include descendants/)
  await click('Copy diagnostics', dialog())
  assert.match(copied, /Containing folder: \/project\/ui\/node_modules/)
  assert.match(copied, /Coverage is partial/)
  assert.match(copied, /Show ignored files: true/)
  assert.match(copied, /Indexing exclusions \(global\): .cide\/worktrees, .cide\/tasks/)
  copyFails = true
  await click('Copy diagnostics', dialog())
  assert.match(dialog().textContent, /Could not copy diagnostics/)
  copyFails = false
  await click('Settings', dialog())
  assert.equal(dialog(), null)
  assert.equal(calls.filter((c) => c.command === 'tab_open_settings').length, 2)

  reply = async () => { throw { kind: 'io', detail: 'read failed' } }
  await click('Details')
  assert.match(dialog().textContent, /Could not load diagnostics/)
  assert.equal(button('Copy diagnostics', dialog()).disabled, true)
  reply = async () => ({ ...report, folders: [], refusal: { ...report.refusal, reason: 'totalBudget', allocation: 'overhead', path: null, folder: null } })
  await click('Refresh', dialog())
  assert.match(dialog().textContent, /Combined memory limit reached/)
  assert.match(dialog().textContent, /No path \(infrastructure allocation\)/)
  assert.match(dialog().textContent, /No indexed folders available/)

  let resolveOld
  reply = () => new Promise((resolve) => { resolveOld = resolve })
  await click('Refresh', dialog())
  assert.match(dialog().textContent, /Loading indexing diagnostics/)
  reply = async () => ({ ...report, generation: 2, refusal: null, files: 222 })
  await render('p', { generation: 2, indexLimit: null })
  assert.match(dialog().textContent, /222 files/)
  await act(async () => resolveOld(report))
  assert.match(dialog().textContent, /222 files/, 'late generation-one result cannot replace generation two')
  assert.match(dialog().textContent, /No memory limit recorded/)
  await act(async () => dialog().dispatchEvent(new dom.window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true })))
  assert.equal(dialog(), null)

  await render()
  reply = () => new Promise((resolve) => { resolveOld = resolve })
  await click('Details')
  await render('q')
  assert.equal(dialog(), null, 'switching projects dismisses diagnostics')
  await act(async () => resolveOld(report))
  assert.equal(dialog(), null)

  reply = async () => structuredClone(report)
  await click('Details')
  let finishRestart
  restartReply = () => new Promise((resolve) => { finishRestart = resolve })
  await click('Restart indexing', dialog())
  assert.deepEqual(calls.find((c) => c.command === 'fs_restart_index')?.args, { project: 'q', generation: 1 })
  assert.match(dialog().textContent, /Rebuilding the project index/)
  assert.equal(button('Indexing…', dialog()).disabled, true)
  await click('Indexing…', dialog())
  assert.equal(calls.filter((c) => c.command === 'fs_restart_index').length, 1, 'duplicate clicks are ignored')
  reply = async () => ({ ...report, generation: 2, refusal: null, files: 42 })
  await act(async () => finishRestart({ ...status, generation: 2, indexLimit: null }))
  assert.match(dialog().textContent, /42 files/)
  assert.match(dialog().textContent, /No memory limit recorded/)
  assert.equal(button('Restart indexing', dialog()).disabled, false, 'a completed rebuild reloads diagnostics')

  restartReply = async () => { throw new Error('worker failed') }
  await click('Restart indexing', dialog())
  assert.match(dialog().textContent, /Could not restart indexing: worker failed/)
  assert.equal(button('Restart indexing', dialog()).disabled, false, 'a failed restart can be retried')
  restartReply = async () => ({ ...status, generation: 3 })
  reply = async () => ({ ...report, generation: 3 })
  await click('Restart indexing', dialog())
  assert.doesNotMatch(dialog().textContent, /Could not restart indexing/)
  assert.match(dialog().textContent, /Project memory limit reached/, 'an insufficient budget remains visible after retry')

  await render('q', { generation: 3, indexing: true })
  assert.equal(button('Indexing…', dialog()).disabled, true, 'a walk started elsewhere cannot be duplicated')
  await render('q', { generation: 3, indexing: false })
  restartReply = () => new Promise((resolve) => { finishRestart = resolve })
  await click('Restart indexing', dialog())
  await render('another')
  await act(async () => finishRestart({ ...status, generation: 4 }))
  assert.equal(dialog(), null, 'finishing a restart does not reopen a dismissed dialog')
  console.log('index diagnostics: Settings, dialog, copy, refresh, restart, failures and stale responses passed')
} finally {
  if (root) await act(async () => root.unmount())
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
