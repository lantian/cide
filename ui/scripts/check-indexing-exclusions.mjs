import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

const ui = resolve(import.meta.dirname, '..')
mkdirSync(join(ui, 'node_modules/.cache'), { recursive: true })
const out = mkdtempSync(join(ui, 'node_modules/.cache/indexing-exclusions-'))
const dom = new JSDOM('<!doctype html><div id="root"></div>', { url: 'https://cide.test' })
for (const key of ['window', 'document', 'HTMLElement', 'Element', 'Node']) globalThis[key] = dom.window[key]
globalThis.IS_REACT_ACT_ENVIRONMENT = true
const { createRoot } = await import('react-dom/client')
const root = createRoot(document.getElementById('root'))
const explorer = { indexMemoryMiB: 512, totalIndexMemoryMiB: 2048, showHiddenFiles: true, showIgnoredFiles: true, indexExcludedFolders: ['.cide/worktrees', '.cide/tasks'] }
const writes = []
let override = null
let getProject = async () => override
window.__TAURI_INTERNALS__ = { invoke: async (command, args) => {
  if (command === 'project_indexing_get') return getProject()
  if (command === 'settings_get') return { explorer }
  if (command === 'project_indexing_set') {
    if (args.exclusions?.includes('../outside')) throw new Error('Invalid folder exclusion: parent traversal')
    writes.push({ command, args })
    override = args.exclusions
    return override
  }
  if (command === 'settings_set') { writes.push({ command, args }); return args.patch }
  throw new Error(`Unexpected ${command}`)
} }

try {
  await build({ configFile: false, root: ui, logLevel: 'error', resolve: { alias: { '@': join(ui, 'src') } },
    build: { ssr: join(ui, 'src/settings/IndexingExclusions.tsx'), outDir: out, rollupOptions: { output: { entryFileNames: 'form.mjs' } } } })
  const { IndexingExclusions } = await import(pathToFileURL(join(out, 'form.mjs')))
  const render = async (project = 'p') => act(async () => root.render(React.createElement(IndexingExclusions, { project, explorer })))
  const field = () => document.querySelector('textarea')
  const button = (label) => [...document.querySelectorAll('button')].find((b) => b.textContent === label)
  const click = async (label) => { assert.ok(button(label), label); await act(async () => button(label).click()) }
  const type = async (text) => act(async () => {
    Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, 'value').set.call(field(), text)
    field().dispatchEvent(new dom.window.Event('input', { bubbles: true }))
  })
  await render()
  assert.equal(field().value, '.cide/worktrees\n.cide/tasks')
  await type(' **/node_modules\n\n build ')
  await click('Save exclusions')
  assert.deepEqual(writes.pop(), { command: 'settings_set', args: { patch: { explorer: { ...explorer, indexExcludedFolders: ['**/node_modules', 'build'] } } } })
  await click('This project')
  assert.match(document.body.textContent, /Using global exclusions/)
  await type('')
  await click('Save exclusions')
  assert.deepEqual(writes.pop(), { command: 'project_indexing_set', args: { project: 'p', exclusions: [] } })
  assert.equal(field().value, '')
  assert.match(document.body.textContent, /replaces the global/)
  await click('Use global exclusions')
  assert.deepEqual(writes.pop().args, { project: 'p', exclusions: null })
  assert.equal(field().value, '.cide/worktrees\n.cide/tasks')
  await type('../outside')
  await click('Save exclusions')
  assert.match(document.querySelector('[role="alert"]').textContent, /parent traversal/)
  assert.equal(field().value, '../outside', 'invalid drafts stay editable')
  let release
  getProject = () => new Promise((resolve) => { release = resolve })
  await render('another')
  assert.ok(field().disabled)
  await click('Global')
  await act(async () => release(['stale']))
  assert.equal(field().value, '.cide/worktrees\n.cide/tasks', 'stale project responses cannot overwrite global rules')
  getProject = async () => { throw new Error('Preferences unavailable') }
  await click('This project')
  assert.ok(field().disabled)
  assert.ok(button('Save exclusions').disabled)
  assert.match(document.querySelector('[role="alert"]').textContent, /Preferences unavailable/)
  getProject = async () => ['recovered']
  await click('Retry loading exclusions')
  assert.equal(field().value, 'recovered')
  assert.equal(field().disabled, false)
  await render(null)
  assert.equal(button('This project'), undefined)
  assert.equal(button('Global').getAttribute('aria-checked'), 'true')
  console.log('indexing exclusions: global/project save, empty override, reset, validation and stale responses passed')
} finally {
  await act(async () => root.unmount())
  rmSync(out, { recursive: true, force: true })
  dom.window.close()
}
