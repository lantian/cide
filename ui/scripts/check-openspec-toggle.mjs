import { strict as assert } from 'node:assert'
import { mkdtempSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'
const ui = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(ui, 'node_modules/.cache/spec-toggle-'))
const dom = new JSDOM('<div id="root"></div>', { url: 'https://cide.test' })
for (const name of ['window', 'document', 'HTMLElement', 'Element', 'Node']) globalThis[name] = dom.window[name]
globalThis.IS_REACT_ACT_ENVIRONMENT = true
const { createRoot } = await import('react-dom/client')
let root
try {
  await build({ root: ui, configFile: false, logLevel: 'error', resolve: { alias: { '@': join(ui, 'src') } },
    plugins: [{ name: 'toggle-entry', resolveId(id) { if (id.endsWith('toggle-entry')) return '\0toggle-entry' },
      load(id) { if (id === '\0toggle-entry') return `export { SpecTabView } from '${join(ui, 'src/sidebar/OpenSpecPanel/SpecTab.tsx')}'; export { TAB_STORIES } from '${join(ui, 'src/sidebar/OpenSpecPanel/fixture.ts')}'; export { readyActs, stageOf } from '${join(ui, 'src/sidebar/OpenSpecPanel/model.ts')}';` } }],
    build: { ssr: 'toggle-entry', outDir: out, rollupOptions: { output: { entryFileNames: 'test.mjs' } } },
  })
  const { SpecTabView, TAB_STORIES, readyActs, stageOf } = await import(pathToFileURL(join(out, 'test.mjs')))
  const story = TAB_STORIES['tab-ready']
  let view = { ...story.view, tasks: [{ description: 'Last task', done: false }], total: 1, completed: 0 }
  let saving = false, toggles = [], archived = false
  const render = () => root.render(React.createElement(SpecTabView, { ...story, view, taskSaving: saving,
    readyActs: stageOf(view) === 'ready' ? readyActs(false) : [],
    onTaskToggle: (index, done) => toggles.push({ index, done }), onReadyAct: () => { archived = true },
  }))
  root = createRoot(document.getElementById('root'))
  await act(render)
  const box = () => document.querySelector('input[type=checkbox]')
  const archive = () => [...document.querySelectorAll('button')].find(b => b.textContent === 'Archive')
  await act(() => document.querySelector('[data-audit=specTabStep] span').click())
  assert.equal(toggles.length, 0, 'task text does not toggle')
  await act(() => box().click())
  assert.deepEqual(toggles, [{ index: 0, done: true }])
  saving = true
  await act(render)
  assert.equal(box().disabled, true)
  assert.equal(archive(), undefined, 'no premature archive while saving')
  saving = false
  view = { ...view, tasks: [{ description: 'Last task', done: true }], completed: 1 }
  await act(render)
  assert.equal(box().checked, true)
  assert.ok(archive(), 'last task exposes Archive on refreshed view')
  await act(() => archive().click())
  assert.ok(archived, 'Archive delegates to existing action')
  await act(() => box().click())
  assert.deepEqual(toggles.at(-1), { index: 0, done: false })
  view = { ...view, tasks: [{ description: 'Last task', done: false }], completed: 0 }
  await act(render)
  assert.equal(archive(), undefined, 'unchecking removes completion actions')
  console.log('OpenSpec task toggle interaction checks passed')
} finally {
  if (root) await act(() => root.unmount())
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
