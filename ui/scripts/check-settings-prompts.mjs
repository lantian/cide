// Mount the real editors: blur must not write, failed saves must not lose writing, and a late
// reply from an unmounted editor must not erase a newer draft for the same project.
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { JSDOM } from 'jsdom'
import React, { act, useState } from 'react'
import ts from 'typescript'
import { build } from 'vite'

mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync('node_modules/.cache/cide-settings-prompts-')
const dom = new JSDOM('<!doctype html><div id="root"></div>', {
  url: 'https://cide.test',
  pretendToBeVisual: true,
})
for (const name of ['window', 'document', 'HTMLElement', 'Element', 'Node', 'MutationObserver']) {
  globalThis[name] = dom.window[name]
}
globalThis.self = dom.window
globalThis.IS_REACT_ACT_ENVIRONMENT = true
const { createRoot } = await import('react-dom/client')
let root
try {
  await build({
    logLevel: 'error',
    build: { ssr: 'src/settings/PromptEditors.tsx', outDir: out },
  })
  const { PromptEditors } = await import(pathToFileURL(resolve(out, 'PromptEditors.js')).href)
  const defaults = {
    taskStartPrompt: 'Work on task {task_id}: {task_title}. Read it with {cide_task_get}.',
    workerInstructions: 'Default tracker rules.\n\nUse {cide_task_comment}.',
  }
  let stored = { ...defaults }
  let nextSave = 'success'
  let reply
  const calls = []
  async function save(patch) {
    calls.push(patch)
    if (nextSave === 'failure') throw new Error('Read-only project')
    if (nextSave === 'deferred') await new Promise((done) => { reply = done })
    stored = {
      ...stored,
      taskStartPrompt: patch.taskStartPrompt.trim().replace(/\s+/g, ' ') || defaults.taskStartPrompt,
      workerInstructions: patch.workerInstructions.trim() || defaults.workerInstructions,
    }
    return { ...stored }
  }
  function Host({ project }) {
    const [config, setConfig] = useState({ ...stored })
    return React.createElement(PromptEditors, {
      project,
      config,
      save: async (patch) => {
        const next = await save(patch)
        setConfig(next)
        return next
      },
    })
  }
  root = createRoot(document.getElementById('root'))
  async function mount(project = 'project-a') {
    await act(async () => root.render(React.createElement(Host, { key: project, project })))
  }
  async function leave() {
    await act(async () => root.render(null))
  }
  const fields = () => [...document.querySelectorAll('textarea')]
  const button = (text) => [...document.querySelectorAll('button')].find((item) => item.textContent === text)
  async function edit(index, value) {
    await act(async () => {
      const field = fields()[index]
      Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, 'value').set.call(field, value)
      field.dispatchEvent(new dom.window.Event('input', { bubbles: true }))
      field.dispatchEvent(new dom.window.FocusEvent('focusout', { bubbles: true }))
    })
  }
  async function click(text) {
    await act(async () => button(text).click())
  }

  await mount()
  assert.deepEqual(fields().map((field) => field.value), Object.values(defaults), 'backend defaults are visible')
  assert.equal(button('Save prompts').disabled, true)
  assert.equal(calls.length, 0, 'mount does not freeze defaults into the project')
  assert.equal(document.querySelector('label').htmlFor, fields()[0].id, 'fields have accessible labels')

  await edit(0, 'Implement {task_id}\ncarefully')
  await edit(1, 'Custom rules.\n\nReport once.')
  assert.equal(calls.length, 0, 'typing and blur do not save')
  assert.equal(button('Save prompts').disabled, false)
  await leave()
  await mount('project-b')
  assert.deepEqual(fields().map((field) => field.value), Object.values(defaults), 'drafts do not cross projects')
  await mount()
  assert.equal(fields()[0].value, 'Implement {task_id}\ncarefully', 'unsaved draft survives section switch')

  nextSave = 'failure'
  await click('Save prompts')
  assert.match(document.querySelector('[role="status"][data-tone="warn"]').textContent, /Read-only project/)
  assert.equal(fields()[0].value, 'Implement {task_id}\ncarefully', 'failure preserves draft')
  nextSave = 'success'
  await click('Save prompts')
  assert.deepEqual(calls.at(-1), {
    taskStartPrompt: 'Implement {task_id}\ncarefully',
    workerInstructions: 'Custom rules.\n\nReport once.',
  }, 'both prompts save in one patch')
  assert.equal(fields()[0].value, 'Implement {task_id} carefully', 'show backend normalization after save')
  assert.equal(button('Save prompts').disabled, true)
  assert.equal(document.querySelector('[role="status"][data-tone="warn"]'), null)
  await leave()
  stored.taskStartPrompt = 'Changed on disk'
  await mount()
  assert.equal(fields()[0].value, 'Changed on disk', 'clean cache does not override a fresh config')

  nextSave = 'failure'
  await click('Reset to defaults')
  assert.equal(fields()[0].value, 'Changed on disk', 'failed reset keeps saved content')
  nextSave = 'success'
  await click('Reset to defaults')
  assert.deepEqual(calls.at(-1), { taskStartPrompt: '', workerInstructions: '' })
  assert.deepEqual(fields().map((field) => field.value), Object.values(defaults), 'reset displays returned defaults')

  await edit(0, 'An older draft')
  nextSave = 'deferred'
  const beforeSave = calls.length
  await act(async () => {
    const saveButton = button('Save prompts')
    saveButton.click()
    saveButton.click()
  })
  assert.equal(calls.length, beforeSave + 1, 'repeated clicks cannot send the same save twice')
  assert.ok(fields().every((field) => field.disabled), 'saving disables edits in this instance')
  assert.ok(button('Reset to defaults').disabled)
  await leave()
  await mount()
  await edit(0, 'A newer draft')
  await act(async () => reply())
  assert.equal(fields()[0].value, 'A newer draft', 'late reply cannot overwrite a mounted newer draft')
  await leave()
  await mount()
  assert.equal(fields()[0].value, 'A newer draft', 'late reply cannot erase the newer cached draft')
  nextSave = 'success'
  await click('Save prompts')
  assert.equal(stored.taskStartPrompt, 'A newer draft')

  const code = ts.transpileModule(readFileSync('src/settings/settingsIndex.ts', 'utf8'), {
    compilerOptions: { module: ts.ModuleKind.ESNext },
  }).outputText
  const { searchSettings } = await import(`data:text/javascript;base64,${Buffer.from(code).toString('base64')}`)
  for (const query of ['task prompt', 'dispatch template', 'plan template', 'worker instructions']) {
    assert.ok(searchSettings(query).some((hit) => hit.section === 'agents' && hit.tab === 'prompts'), query)
  }
  console.log('settings prompts: visible defaults, explicit save/reset, drafts, failures, late replies and search passed')
} finally {
  if (root) await act(async () => root.unmount())
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
