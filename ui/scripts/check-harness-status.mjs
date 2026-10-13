/** Mount the real status bar and version hook with delayed IPC replies. */
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

const ui = resolve(import.meta.dirname, '..')
mkdirSync(join(ui, 'node_modules/.cache'), { recursive: true })
const out = mkdtempSync(join(ui, 'node_modules/.cache/harness-status-'))
const dom = new JSDOM('<!doctype html><div id="root"></div>', { url: 'https://cide.test' })
for (const key of ['window', 'document', 'HTMLElement', 'Element', 'Node']) globalThis[key] = dom.window[key]
globalThis.IS_REACT_ACT_ENVIRONMENT = true
const queries = []
globalThis.__harnessQueries = queries
const { createRoot } = await import('react-dom/client')
const root = createRoot(document.getElementById('root'))
try {
  await build({
    configFile: false, root: ui, logLevel: 'error',
    resolve: { alias: { '@': join(ui, 'src') } },
    plugins: [{
      name: 'harness-status-fixture', enforce: 'pre',
      resolveId(source, importer) {
        if (source.endsWith('harness-status-check')) return '\0harness-status-check'
        if (source.endsWith('/ipc/client')) return '\0harness-ipc'
        if (importer?.endsWith('/chrome/StatusBar.tsx') && /^\.\/(BranchSelector|GitOpIndicator)$/.test(source)) return '\0harness-children'
      },
      load(id) {
        if (id === '\0harness-ipc') return `export const settings = { harnessVersion: (project) => new Promise((resolve, reject) => globalThis.__harnessQueries.push({ project, resolve, reject })) }`
        if (id === '\0harness-children') return 'export const BranchSelector = () => null; export const GitOpIndicator = () => null'
        if (id === '\0harness-status-check') return `
          import React from 'react'
          import { StatusBar } from '@/chrome/StatusBar'
          import { useHarnessVersion } from '@/chrome/useHarnessVersion'
          import { statusHarnessReadout } from '@/chrome/harnessReadout'
          import { formatClaude } from '@/store/statusFormat'
          export * from '@/chrome/harnessReadout'
          export function TestHost({ project, settings, overrides, payload, editorFocused }) {
            const fallback = useHarnessVersion(project, settings, overrides)
            return React.createElement(StatusBar, { harnessReadout: statusHarnessReadout(formatClaude(payload), fallback), editorFocused })
          }
        `
      },
    }],
    build: { ssr: 'harness-status-check', outDir: out, rollupOptions: { output: { entryFileNames: 'status.mjs' } } },
  })
  const { TestHost, defaultHarnessReadout } = await import(pathToFileURL(join(out, 'status.mjs')))
  let props = {
    project: 'one', settings: null, overrides: {}, payload: undefined, editorFocused: true,
  }
  const render = async (patch = {}) => {
    props = { ...props, ...patch }
    await act(async () => root.render(React.createElement(TestHost, props)))
  }
  const readout = () => document.querySelector('[data-audit="harnessReadout"]')
  const text = () => readout().textContent
  const reply = async (query, info) => act(async () => query.resolve(info))
  const codex = { harness: 'codex', binary: '/custom/codex', version: 'codex-cli 0.114.0-beta.2' }
  const settings = {
    consoleHarness: 'codex',
    claude: { cli: { binary: 'claude' } },
    codex: { cli: { binary: '/custom/codex' } },
    opencode: { cli: { binary: 'opencode' } },
  }

  await render()
  assert.equal(queries.length, 0, 'wait for bootstrap before probing')
  assert.equal(text(), 'Harness · …')
  await render({ settings })
  assert.equal(queries.length, 1)
  assert.equal(queries[0].project, 'one')
  await reply(queries[0], codex)
  assert.equal(text(), 'Codex · 0.114.0-beta.2', 'editor focus uses the configured default')
  assert.equal(readout().title, 'Default console harness: Codex\nBinary: /custom/codex')
  await render({ editorFocused: false })
  assert.equal(text(), 'Codex · 0.114.0-beta.2', 'focus does not choose the fallback harness')
  await render({ payload: { model: { display_name: 'Opus 5' } } })
  assert.equal(text(), 'claude · Opus 5', 'a live Claude session wins over the Codex default')
  assert.equal(readout().title, 'Claude session')
  await render({ payload: { harness: 'codex', model: { display_name: 'gpt-6' }, rate_limit: { used_percent: 12 } } })
  assert.equal(text(), 'codex · gpt-6 · limit 12%')
  assert.equal(readout().title, 'Codex session')
  await render({ payload: {} })
  assert.equal(text(), 'Codex · 0.114.0-beta.2')
  assert.equal(queries.length, 1, 'focus and status events do not probe')
  await render({ settings: { ...settings, uiFontSize: 14, codex: { cli: { ...settings.codex.cli, args: ['--help'] } } } })
  assert.equal(queries.length, 1, 'unrelated settings and launch arguments do not probe')

  await render({ settings: { ...settings, codex: { cli: { binary: '/new/codex' } } } })
  const oldBinary = queries.at(-1)
  assert.equal(text(), 'Harness · …', 'hide the previous binary version immediately')
  await render({ settings: { ...settings, consoleHarness: 'claude' } })
  const current = queries.at(-1)
  await reply(current, { harness: 'claude', binary: 'claude', version: '2.1.291 (Claude Code)' })
  assert.equal(text(), 'Claude Code · 2.1.291')
  await reply(oldBinary, { ...codex, binary: '/new/codex', version: 'codex-cli 0.999.0' })
  assert.equal(text(), 'Claude Code · 2.1.291', 'a superseded request cannot overwrite the current version')

  await render({ settings, overrides: { '/canonical/project': { consoleHarness: 'opencode', opencode: { binary: '/project/opencode' } } } })
  const projectOverride = queries.at(-1)
  await render({ project: 'two' })
  assert.equal(queries.at(-1).project, 'two')
  await reply(queries.at(-1), codex)
  await reply(projectOverride, { harness: 'opencode', binary: '/project/opencode', version: '1.2.3' })
  assert.equal(text(), 'Codex · 0.114.0-beta.2', 'a delayed previous project reply is ignored')
  await render({ project: 'one' })
  await reply(queries.at(-1), { harness: 'opencode', binary: '/project/opencode', version: '1.2.3' })
  assert.equal(text(), 'OpenCode · 1.2.3', 'the backend resolves the project override')
  await render({ overrides: {} })
  await reply(queries.at(-1), codex)
  assert.equal(text(), 'Codex · 0.114.0-beta.2', 'removing the override restores global inheritance')
  await render({ project: null })
  assert.equal(queries.at(-1).project, null)
  await reply(queries.at(-1), { ...codex, version: null })
  assert.equal(text(), 'Codex · version unavailable')
  assert.equal(readout().title, 'Default console harness: Codex\nBinary: /custom/codex')
  await render({ project: 'closed' })
  await act(async () => queries.at(-1).reject(new Error('project closed')))
  assert.equal(text(), 'Harness · version unavailable', 'IPC failure does not retain another project version')
  assert.equal(defaultHarnessReadout({ harness: 'opencode', binary: 'wrapper', version: ' 2.0.0-rc.1 ' }).text, 'OpenCode · 2.0.0-rc.1')
  assert.equal(defaultHarnessReadout({ ...codex, version: ' ' }).text, 'Codex · version unavailable')

  const app = readFileSync(join(ui, 'src/App.tsx'), 'utf8')
  const slot = app.slice(app.indexOf('<StatusBar'), app.indexOf('revealRoots={revealRoots}', app.indexOf('<StatusBar')))
  assert(slot.includes('harnessReadout={statusHarnessReadout(claudeReadout, defaultHarness)}'), 'App passes the live-first readout to the actual slot')
  assert(!slot.includes('claudeVersion') && !slot.includes('focused?.pane.harness'), 'the fallback never comes from Claude capabilities or pane identity')
  console.log('harness status: ok (versions, live figures, focus, settings, inheritance, projects, stale replies and failures)')
} finally {
  await act(async () => root.unmount())
  dom.window.close()
  delete globalThis.__harnessQueries
  rmSync(out, { recursive: true, force: true })
}
