/** Mount the experimental picker and restoration gate; only IPC and terminal hosting are mocked. */
import assert from 'node:assert/strict'
import { mkdtempSync, mkdirSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'
import react from '@vitejs/plugin-react'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

const ui = resolve(import.meta.dirname, '..')
mkdirSync(join(ui, 'node_modules/.cache'), { recursive: true })
const out = mkdtempSync(join(ui, 'node_modules/.cache/peer-chat-'))
const dom = new JSDOM('<!doctype html><div id="root"></div>', { url: 'https://cide.test', pretendToBeVisual: true })
for (const name of ['window', 'document', 'location', 'HTMLElement', 'Element', 'Node', 'KeyboardEvent', 'MouseEvent']) globalThis[name] = dom.window[name]
globalThis.self = dom.window
dom.window.HTMLCanvasElement.prototype.getContext = () => null
dom.window.HTMLElement.prototype.scrollIntoView = () => {}
globalThis.IS_REACT_ACT_ENVIRONMENT = true
globalThis.requestAnimationFrame = dom.window.requestAnimationFrame.bind(dom.window)
globalThis.cancelAnimationFrame = dom.window.cancelAnimationFrame.bind(dom.window)
const calls = []
let plan = [], failure = null, receipts = []
window.__TAURI_INTERNALS__ = {
  metadata: { currentWindow: { label: 'shell:test' } },
  invoke: async (command, args) => {
    calls.push({ command, args })
    if (command === 'agents_models') return { models: args.harness === 'opencode' ? ['local/model-one', 'cloud/model-two'] : ['sonnet', 'opus'], problem: null }
    if (command === 'peer_chat_plan') return plan
    if (command === 'peer_chat_receipts') return receipts
    if (command === 'peer_chat_configure' && failure) throw failure
  },
}
let root
try {
  await build({ configFile: false, root: ui, logLevel: 'error',
    resolve: { alias: [
      { find: '@/store/workspace', replacement: '\0peer-store' },
      { find: '@/layout/paneHosts', replacement: '\0peer-host' },
      { find: './TerminalPane', replacement: '\0peer-terminal' },
      { find: './paneFocus', replacement: '\0peer-focus' },
      { find: '@', replacement: join(ui, 'src') },
    ] },
    plugins: [{ name: 'peer-fixtures', enforce: 'pre',
      resolveId(id, importer) {
        if (id.startsWith('\0peer-')) return id
        if (id === '@/store/workspace' || /\/store\/workspace(?:\.ts)?$/.test(id)) return '\0peer-store'
        if (id === '@/layout/paneHosts' || /\/layout\/paneHosts(?:\.ts)?$/.test(id)) return '\0peer-host'
        if (importer?.endsWith('/PeerChatPane.tsx') && (id === './TerminalPane' || /\/TerminalPane(?:\.tsx)?$/.test(id))) return '\0peer-terminal'
        if (importer?.endsWith('/PeerChatPane.tsx') && (id === './paneFocus' || /\/paneFocus(?:\.ts)?$/.test(id))) return '\0peer-focus'
      },
      load(id) {
        if (id === '\0peer-store') return `export const useWorkspace = fn => fn({boot:null}); useWorkspace.getState=()=>({synced:async()=>{}});`
        if (id === '\0peer-host') return `export const peekHost=()=>null;`
        if (id === '\0peer-focus') return `export const focusPaneDom=()=>{};`
        if (id === '\0peer-terminal') return `import {jsx} from 'react/jsx-runtime'; export const TerminalPane=p=>jsx('div', {'data-terminal':p.pane.id, 'data-restore':JSON.stringify(p.restore), 'data-fresh':p.recovery});`
      },
    }, react()],
    build: { ssr: join(ui, 'src/panes/PeerChatPane.tsx'), outDir: out, emptyOutDir: false },
  })
  const { PeerChatPane } = await import(pathToFileURL(join(out, 'PeerChatPane.js')).href)
  const { createRoot } = await import('react-dom/client')
  root = createRoot(document.getElementById('root'))
  let key = 0
  const chat = { main: 'main', peer: 'peer', mainHarness: 'codex', selection: null, paused: true, starting: false }
  const props = (main, pair = chat, session = null) => ({ project: 'project', pane: { id: main ? 'main' : 'peer', kind: 'claude', role: 'auxiliary', session }, tab: { id: 'tab', peerChat: pair } })
  const mount = async (p) => {
    await act(async () => root.render(React.createElement(PeerChatPane, { ...p, key: ++key })))
    await act(async () => { await new Promise(resolve => setTimeout(resolve, 20)) })
  }
  const button = text => [...document.querySelectorAll('button')].find(b => b.textContent.includes(text))
  const click = async b => { assert.ok(b); await act(async () => { b.click(); await new Promise(resolve => setTimeout(resolve, 20)) }) }
  await mount(props(true))
  assert(document.body.textContent.includes('Codex'))
  assert.equal(document.querySelector('[data-terminal]'), null)
  assert.equal(calls.filter(c => c.command === 'agents_models').length, 0)
  await mount(props(false))
  assert(document.body.textContent.includes('Choose your peer'))
  assert.equal(calls.filter(c => c.command === 'peer_chat_configure').length, 0)
  await click(document.querySelector('[role="combobox"]'))
  const harnessOptions = [...document.querySelectorAll('[role="option"]')]
  assert.deepEqual(harnessOptions.map(o => o.textContent), ['Claude Code', 'Codex', 'OpenCode'])
  await click(harnessOptions[2])
  await click(document.querySelectorAll('[role="combobox"]')[1])
  await click([...document.querySelectorAll('[role="option"]')].find(o => o.textContent === 'local'))
  assert.equal(button('Start two-agent chat').disabled, true)
  const input = document.querySelector('input')
  await act(async () => {
    Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, 'value').set.call(input, 'model-one')
    input.dispatchEvent(new dom.window.Event('input', { bubbles: true }))
  })
  await click(button('Start two-agent chat'))
  assert.deepEqual(calls.findLast(c => c.command === 'peer_chat_configure').args, { project: 'project', tab: 'tab', selection: { harness: 'opencode', model: 'local/model-one' } })
  failure = 'Missing harness binary'
  await mount(props(false))
  await click(button('Start two-agent chat'))
  assert(document.body.textContent.includes(failure), document.body.textContent + ' configure calls=' + calls.filter(c=>c.command==='peer_chat_configure').length)
  failure = null
  const configured = { ...chat, selection: { harness: 'opencode', model: 'local/model-one' } }
  await mount(props(true, { ...configured, starting: true }))
  assert(document.body.textContent.includes('Starting both agents'))
  assert.equal(document.querySelector('[data-terminal]'), null)
  plan = [{ pane: 'main', restore: { kind: 'resumable', session: 'saved-native' } }]
  await mount(props(true, configured, 'saved-session'))
  assert(document.body.textContent.includes('Conversation paused'))
  assert(document.querySelector('[data-terminal]').dataset.restore.includes('saved-native'))
  await click(button('Resume conversation'))
  assert.equal(calls.findLast(c => c.command === 'peer_chat_pause').args.paused, false)
  receipts = [{ id: 'outgoing', sender: 'main', status: 'delivered', detail: 'Accepted by the peer' }]
  await mount(props(true, configured, 'saved-session'))
  assert(document.body.textContent.includes('Main → Peer: delivered: Accepted by the peer'))
  assert(document.body.textContent.includes('Peer → Main: No reply sent yet'))
  receipts.push({ id: 'reply', sender: 'peer', status: 'delivered', detail: 'Accepted by the main' })
  await mount(props(false, configured, 'saved-session'))
  assert(document.body.textContent.includes('Main → Peer: delivered: Accepted by the peer'))
  assert(document.body.textContent.includes('Peer → Main: delivered: Accepted by the main'))
  plan = [{ pane: 'peer', restore: { kind: 'missing' } }]
  await mount(props(false, configured, 'missing-session'))
  assert(document.body.textContent.includes('could not be restored'))
  assert.equal(document.querySelector('[data-terminal]'), null)
  await click(button('Start a fresh peer'))
  assert.equal(document.querySelector('[data-terminal]').dataset.fresh, 'fresh')
  console.log('peer chat: picker, native selection, startup failure, directional receipts, paused resume and explicit fresh recovery pass')
} finally {
  if (root) await act(async () => root.unmount())
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
