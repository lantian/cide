// Exercise the shared preference hook across workflows, projects and delayed saves.
import { strict as assert } from 'node:assert'
import { mkdtempSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

const ui = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(ui, 'node_modules/.cache/spec-plan-'))
const dom = new JSDOM('<div id="root"></div>', { url: 'https://cide.test' })
for (const key of ['window', 'document', 'HTMLElement', 'Element', 'Node']) globalThis[key] = dom.window[key]
globalThis.IS_REACT_ACT_ENVIRONMENT = true
const { createRoot } = await import('react-dom/client')
const defaults = () => ({ applyInWorktree: true, proposePlanMode: false, explorePlanMode: false, applyPlanMode: false })
const prefs = { a: defaults(), b: defaults() }
let fail = false, delayed = null
window.planTest = {
  read: async (p) => ({ ...prefs[p] }),
  write: async (p, patch) => {
    if (fail) throw Error('save failed')
    if (delayed) await delayed
    return Object.assign(prefs[p], patch)
  },
}
let root
try {
  await build({
    root: ui, configFile: false, logLevel: 'error',
    resolve: { alias: { '@': join(ui, 'src') } },
    plugins: [{
      name: 'plan-test-ipc', enforce: 'pre',
      resolveId(source) {
        if (source === '@/ipc/client' || source === join(ui, 'src/ipc/client')) return '\0plan-ipc'
      },
      load(id) {
        if (id === '\0plan-ipc') return 'export const specSessions = { settings: (...a) => window.planTest.read(...a), setSettings: (...a) => window.planTest.write(...a) };'
      },
    }],
    build: { ssr: join(ui, 'src/sidebar/OpenSpecPanel/PlanMode.tsx'), outDir: out,
      rollupOptions: { output: { entryFileNames: 'plan.mjs' } } },
  })
  const { usePlanMode } = await import(pathToFileURL(join(out, 'plan.mjs')))
  let current
  function Probe(props) { current = usePlanMode(props.project, props.op, props.open); return null }
  root = createRoot(document.getElementById('root'))
  const render = async (project, op, open = true) => {
    await act(async () => root.render(React.createElement(Probe, { project, op, open })))
  }
  const change = async (value) => { await act(async () => current.change(value)) }
  await render('a', 'propose')
  assert.equal(current.value, false)
  await change(true)
  await render('a', 'explore')
  assert.equal(current.value, false)
  await change(true)
  await render('a', 'apply')
  assert.equal(current.value, false)
  await change(true)
  assert.deepEqual(prefs.a, { applyInWorktree: true, proposePlanMode: true, explorePlanMode: true, applyPlanMode: true })
  await render('a', 'apply', false)
  await render('a', 'apply')
  assert.equal(current.value, true)
  await render('b', 'apply')
  assert.equal(current.value, false)
  fail = true
  await change(true)
  assert.equal(current.value, false)
  assert.match(current.error, /save failed/)
  fail = false
  let release
  delayed = new Promise((resolve) => { release = resolve })
  await act(async () => current.change(true))
  assert.equal(current.ready, false)
  await render('b', 'explore')
  await act(async () => release())
  assert.equal(current.value, false, 'late Apply save cannot overwrite Explore')
  assert.equal(current.ready, true)
  await render('a', 'propose')
  assert.equal(current.value, true)
  await change(false)
  assert.equal(prefs.a.proposePlanMode, false)
  assert.equal(prefs.a.explorePlanMode, true)
  console.log('OpenSpec plan preferences: workflow/project isolation, reopen, failure and stale-save checks passed')
} finally {
  if (root) await act(async () => root.unmount())
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
