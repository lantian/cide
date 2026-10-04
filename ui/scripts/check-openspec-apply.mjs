/**
 * Mount Apply's real dialog, launcher and store in jsdom, like check-harness-settings.mjs.
 * Vite SSR only bundles the entry: React's client reconciler drives focus and events.
 * Mock IPC/session launch; no browser executable, HTTP server or model turn is needed.
 */
import { ok } from 'node:assert/strict'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'
import { flushSync } from 'react-dom'

const ui = resolve(import.meta.dirname, '..')
mkdirSync(join(ui, 'node_modules/.cache'), { recursive: true })
const out = mkdtempSync(join(ui, 'node_modules/.cache/cide-apply-check-'))
const dom = new JSDOM('<!doctype html><button id="outside">Underlying control</button><div id="root"></div>', {
  url: 'https://cide.test', pretendToBeVisual: true,
})
for (const name of ['window', 'document', 'HTMLElement', 'Element', 'Node']) globalThis[name] = dom.window[name]
globalThis.IS_REACT_ACT_ENVIRONMENT = true
// jsdom has no layout; Select scrolls its active option, which is outside this check's claim.
dom.window.HTMLElement.prototype.scrollIntoView = function () {}
// Import after installing the DOM so React uses its normal input-event handling.
const { createRoot } = await import('react-dom/client')

const mocks = {
  ipc: `export const specSessions = {
    settings: async () => ({ applyInWorktree: false }),
    harnesses: async () => [{ harness: 'codex', unavailable: null }],
  };
  export const settings = { effective: async () => ({ consoleHarness: 'codex' }) };`,
  agents: `import { create } from 'zustand';
    export const useAgents = create(() => ({ project: 'test-project', roster: { kind: 'ready', agents: [] } }));`,
  runs: `export function startSession(project, request) {
    window.applyCheck.launches.push({ project, request });
    return window.applyCheck.launch();
  }`,
  notices: `export function notifyFailure(error) { window.applyCheck.failures.push(String(error)); }`,
}

let checks = 0
window.applyCheck = { launches: [], failures: [], launch: async () => 'mock-run' }
const mock = window.applyCheck
let documentEscapes = 0, parentEscapes = 0
document.addEventListener('keydown', (event) => { if (event.key === 'Escape') documentEscapes++; })
const assert = (condition, message) => { ok(condition, message); checks++ }
// React's act drains launcher/launch promises; no virtual clock or arbitrary sleeps.
const settle = async () => { await act(async () => {}) }
const commit = (update) => act(() => { flushSync(update) })
const dialog = () => document.querySelector('[data-audit="openspecApplyDialog"]')
const instructions = () => dialog().querySelector('textarea')
const outside = document.getElementById('outside')
const key = (key, target = document.activeElement) => {
  const event = new dom.window.KeyboardEvent('keydown', { key, bubbles: true, cancelable: true })
  commit(() => target.dispatchEvent(event))
  return event
}
const escape = (target) => {
  const beforeDocument = documentEscapes, beforeParent = parentEscapes
  const event = key('Escape', target)
  assert(event.defaultPrevented, 'handled Escape prevents default')
  assert(documentEscapes === beforeDocument && parentEscapes === beforeParent,
    'handled Escape does not reach native or React observers outside the portal')
}
const deferred = () => {
  let resolve, reject
  const promise = new Promise((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}
const type = (text) => {
  const target = instructions()
  target.focus()
  Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, 'value').set.call(target, text)
  commit(() => target.dispatchEvent(new dom.window.Event('input', { bubbles: true })))
}

let root
try {
  await build({
    root: ui, configFile: false, logLevel: 'error',
    resolve: { alias: { '@': join(ui, 'src') } },
    plugins: [{
      name: 'apply-dom-fixture', enforce: 'pre',
      resolveId(source, importer) {
        // Vite's alias plugin runs first, so also match the resolved absolute path.
        const aliased = (path) => source === '@/' + path || source === join(ui, 'src', path)
          || source === join(ui, 'src', path) + '.ts'
        const mock = aliased('ipc/client') ? 'ipc'
          : aliased('chrome/notices') ? 'notices'
          : source === '../agentsStore' && importer?.endsWith('/LauncherPicker.tsx') ? 'agents'
          : source === './specRuns' && importer?.endsWith('/ApplyDialog.tsx') ? 'runs' : null
        return mock === null ? null : '\0apply-check:' + mock
      },
      load(id) { return id.startsWith('\0apply-check:') ? mocks[id.slice('\0apply-check:'.length)] : null },
    }],
    build: {
      ssr: join(ui, 'src/sidebar/OpenSpecPanel/ApplyDialog.tsx'), outDir: out,
      rollupOptions: { output: { entryFileNames: 'apply.mjs' } },
    },
  })
  const { ApplyDialog, useApplyDialog } = await import(pathToFileURL(join(out, 'apply.mjs')))

  const open = async (change = 'test-change') => {
    outside.focus()
    commit(() => useApplyDialog.getState().open(change))
    assert(dialog().contains(document.activeElement), 'opening places focus inside Apply')
    await settle()
  }
  const closed = () => assert(!dialog() && useApplyDialog.getState().change === null, 'Escape closes the store and dialog')
  root = createRoot(document.getElementById('root'))
  commit(() => root.render(React.createElement('div', {
    onKeyDown: (event) => { if (event.key === 'Escape') parentEscapes++; },
  }, React.createElement(ApplyDialog, { project: 'test-project' }))))
  assert(!dialog(), 'host starts closed')

  // No wait or click between opening and Escape: focus must be established before paint.
  outside.focus()
  commit(() => useApplyDialog.getState().open('immediate'))
  assert(dialog().contains(document.activeElement), 'immediate opening receives focus')
  escape()
  closed()
  assert(mock.launches.length === 0, 'immediate Escape never launches')

  await open()
  type('keep the old API')
  assert(instructions().value === 'keep the old API', 'instructions accept input')
  escape()
  closed()
  await open()
  assert(instructions().value === '', 'dismissal clears draft instructions on reopening')
  const ordinary = key('a')
  assert(!ordinary.defaultPrevented && !!dialog(), 'other keys retain normal handling')

  const launcher = dialog().querySelector('[role="combobox"]')
  assert(!launcher.disabled, 'launcher control is ready to receive focus')
  launcher.focus()
  assert(document.activeElement === launcher, 'launcher control receives focus')
  // Changing the selected change while visible also reruns the opening effect.
  commit(() => useApplyDialog.getState().open('another-change'))
  assert(document.activeElement === launcher, 'opening preserves focus already inside')
  await settle()
  escape()
  closed()
  assert(mock.launches.length === 0, 'Escape from instructions and launcher never launches')

  await open()
  const dropdown = dialog().querySelector('[role="combobox"]')
  dropdown.focus()
  key('ArrowDown')
  await settle()
  assert(dropdown.getAttribute('aria-expanded') === 'true', 'launcher popup opens normally')
  key('Escape')
  assert(!!dialog() && dropdown.getAttribute('aria-expanded') === 'false', 'nested popup consumes its first Escape')
  escape()
  closed()

  await open()
  type('launch instructions')
  const start = dialog().querySelector('[data-audit="openspecApplyStart"]')
  assert(!start.disabled, 'mocked launcher is ready')
  commit(() => start.click())
  await settle()
  assert(!dialog() && mock.launches.length === 1, 'successful launch closes Apply once')
  assert(mock.launches[0].project === 'test-project'
    && mock.launches[0].request.text === 'launch instructions'
    && mock.launches[0].request.launcher.harness === 'codex', 'launch receives the selected launcher and instructions')
  await open()
  assert(instructions().value === '', 'successful launch clears instructions before reopening')
  escape()
  closed()

  // Hold the same launch pending across repeated Escape, then resolve it normally.
  const pending = deferred()
  mock.launch = () => pending.promise
  await open()
  type('pending instructions')
  commit(() => dialog().querySelector('[data-audit="openspecApplyStart"]').click())
  assert(mock.launches.length === 2, 'pending session starts once')
  dialog().querySelector('[aria-label="Close"]').focus()
  escape()
  escape()
  assert(!!dialog() && useApplyDialog.getState().change === 'test-change'
    && instructions().value === 'pending instructions', 'pending Escape retains the dialog and instructions')
  assert(dialog().querySelector('[data-audit="openspecApplyStart"]').disabled
    && mock.launches.length === 2, 'pending Escape does not duplicate launch')
  await act(async () => pending.resolve('pending-run'))
  assert(!dialog() && mock.launches.length === 2, 'pending Escape does not cancel successful launch')

  const failed = deferred()
  mock.launch = () => failed.promise
  await open()
  assert(instructions().value === '', 'pending success clears instructions')
  type('failed instructions')
  commit(() => dialog().querySelector('[data-audit="openspecApplyStart"]').click())
  await act(async () => failed.reject(Error('mock launch failure')))
  assert(!!dialog() && instructions().value === 'failed instructions'
    && !dialog().querySelector('[data-audit="openspecApplyStart"]').disabled,
    'failure retains instructions and releases the launch guard')
  assert(mock.failures.length === 1 && mock.failures[0].includes('mock launch failure'), 'launch failure is reported')
  instructions().focus()
  escape()
  closed()
  await open()
  assert(instructions().value === '', 'Escape after failure clears instructions')
  escape()
  closed()

  outside.focus()
  const beforeOutside = documentEscapes
  const unhandled = key('Escape')
  assert(!unhandled.defaultPrevented && document.activeElement === outside, 'closed host does not intercept Escape')
  assert(documentEscapes === beforeOutside + 1, 'Escape outside a closed dialog reaches the bubbling observer')
  assert(mock.launches.length === 3, 'reopening and dismissal do not launch again')
  console.log('OpenSpec Apply: ' + checks + ' DOM assertions passed')
} finally {
  if (root) await act(async () => root.unmount())
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
