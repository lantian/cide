/** Mount the actual harness form: project inheritance, independent overrides, and OpenCode. */
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

const ui = resolve(import.meta.dirname, '..')
mkdirSync(join(ui, 'node_modules/.cache'), { recursive: true })
const out = mkdtempSync(join(ui, 'node_modules/.cache/harness-settings-'))
const dom = new JSDOM('<!doctype html><div id="root"></div>', { url: 'https://cide.test' })
for (const key of ['window', 'document', 'HTMLElement', 'Element', 'Node']) globalThis[key] = dom.window[key]
globalThis.IS_REACT_ACT_ENVIRONMENT = true
dom.window.HTMLElement.prototype.scrollIntoView = () => {}
const { createRoot } = await import('react-dom/client')
const root = createRoot(document.getElementById('root'))
try {
  await build({
    configFile: false, root: ui, logLevel: 'error',
    resolve: { alias: { '@': join(ui, 'src') } },
    build: { ssr: join(ui, 'src/settings/HarnessSection.tsx'), outDir: out, rollupOptions: { output: { entryFileNames: 'harness.mjs' } } },
  })
  const { HarnessChoice, HarnessLaunch } = await import(pathToFileURL(join(out, 'harness.mjs')))
  const cli = { binary: '/project/opencode', args: ['--log-level', 'debug'], env: [{ name: 'API_URL', value: 'https://gateway' }], inject: { events: true, mcpConfig: true, instructions: true, resume: true, fork: true } }
  const writes = [], globalWrites = []
  const props = { settings: { consoleHarness: 'opencode', opencode: { cli } }, patch: (patch) => globalWrites.push(patch), editProjectHarness: (edit) => writes.push(edit), cliSupport: null, codexSupport: null }
  const render = async (component, extra = {}) => act(async () => root.render(React.createElement(component, { ...props, ...extra })))
  const click = async (element) => { assert.ok(element); await act(async () => element.click()) }
  const button = (label) => [...document.querySelectorAll('button')].find((element) => element.textContent === label)

  await render(HarnessChoice)
  await click(button('OpenCode'))
  assert.deepEqual(globalWrites.pop(), { consoleHarness: 'opencode' })
  await render(HarnessChoice, { projectHarness: {} })
  assert.equal(button('Use global').getAttribute('aria-checked'), 'true')
  await click(button('OpenCode'))
  assert.deepEqual(writes.pop(), { field: 'consoleHarness', value: 'opencode' })
  await click(button('Use global'))
  assert.deepEqual(writes.pop(), { field: 'consoleHarness', value: null })

  await render(HarnessLaunch, { projectHarness: {} })
  assert.ok(document.querySelector('[data-setting="Inherited launch"]'))
  assert.equal(document.querySelector('[aria-label="OpenCode program"]'), null)
  await click(document.querySelector('[role="switch"][aria-label="Override global launch"]'))
  assert.deepEqual(writes.pop(), { field: 'opencode', value: cli })
  await render(HarnessLaunch, { projectHarness: { opencode: cli } })
  assert.equal(document.querySelector('[aria-label="OpenCode program"]').value, cli.binary)
  assert.equal(document.querySelector('[aria-label="OpenCode argument 1"]').value, '--log-level')
  assert.equal(document.querySelector('[aria-label="OpenCode variable 1 value"]').value, 'https://gateway')
  await click(document.querySelector('[role="switch"][aria-label="Override global launch"]'))
  assert.deepEqual(writes.pop(), { field: 'opencode', value: null })
  const codex = {
    binary: 'codex', args: [], env: [], permissionMode: 'useConfig', permissionProfile: '',
    inject: { hooks: true, mcpConfig: true, developerInstructions: true, resume: true, fork: true, permissions: true, gitPermissions: true, reviewPermissions: true },
  }
  const codexProps = { settings: { consoleHarness: 'codex', codex: { cli: codex } } }
  await render(HarnessLaunch, codexProps)
  const selector = () => document.querySelector('[aria-label="Codex default permissions"]')
  assert.equal(selector().textContent, 'Use Codex config')
  await click(selector())
  const options = [...document.querySelectorAll('[role="option"]')]
  assert.deepEqual(options.map((option) => option.textContent), ['Use Codex config', 'Ask for approval', 'Approve for me', 'Full access', 'Read-only', 'Custom profile'])
  await click(options[2])
  assert.deepEqual(globalWrites.pop(), { codex: { cli: { ...codex, permissionMode: 'approveForMe' } } })
  const custom = { ...codex, permissionMode: 'customProfile' }
  await render(HarnessLaunch, { settings: { consoleHarness: 'codex', codex: { cli: custom } } })
  assert(document.querySelector('[data-setting="Permission profile"]').textContent.includes('Enter a profile name'))
  const profile = document.querySelector('[aria-label="Codex permission profile"]')
  await act(async () => {
    profile.focus()
    Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, 'value').set.call(profile, ' team ')
    profile.dispatchEvent(new dom.window.Event('input', { bubbles: true }))
  })
  await act(async () => profile.blur())
  assert.deepEqual(globalWrites.pop(), { codex: { cli: { ...custom, permissionProfile: 'team' } } })
  await render(HarnessLaunch, { ...codexProps, codexSupport: { binary: 'codex', argNotes: [], envNotes: [], permissionsNote: 'Permission options in Extra arguments override Default permissions.', argv: [] } })
  assert(document.querySelector('[data-setting="Default permissions"]').textContent.includes('Extra arguments override'))
  console.log('harness settings: ok (global/project choice, inheritance, launch override, OpenCode controls, Codex permission presets and custom profiles)')
} finally {
  await act(async () => root.unmount())
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
