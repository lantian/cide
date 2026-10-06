/** Prompt history navigation, speaker matching, a mounted recap and delayed reader races. */
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, mkdirSync, readFileSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

const ui = resolve(import.meta.dirname, '..')
mkdirSync(join(ui, 'node_modules/.cache'), { recursive: true })
const out = mkdtempSync(join(ui, 'node_modules/.cache/user-inputs-'))
let root, dom
const releases = []
try {
  execFileSync('node', ['node_modules/typescript/bin/tsc', 'src/panes/recapModel.ts', 'src/terminal/userInputModel.ts',
    '--outDir', join(out, 'pure'), '--module', 'esnext', '--target', 'es2022', '--moduleResolution', 'bundler',
    '--strict', '--noUncheckedIndexedAccess'], { cwd: ui, stdio: 'inherit' })
  const { selectedOrdinal, navigateInput, mergeInputs } = await import(pathToFileURL(join(out, 'pure/panes/recapModel.js')))
  assert.equal(selectedOrdinal(null, 12), 12)
  assert.equal(navigateInput(null, 12, -1), 11)
  assert.equal(selectedOrdinal(11, 13), 11, 'new prompts hold historical selection')
  assert.equal(navigateInput(11, 12, 1), null, 'next to latest resumes following')
  assert.equal(navigateInput(1, 12, -1), 1)
  assert.equal(selectedOrdinal(null, 0), 0)
  assert.deepEqual(mergeInputs([{ id: '2', ordinal: 2, text: 'same' }], [
    { id: '1', ordinal: 1, text: 'same' }, { id: '2', ordinal: 2, text: 'same' },
  ]).map(x => x.id), ['1', '2'])

  const { inputBlocks, inputLocations, visibleInputOrdinal } = await import(pathToFileURL(join(out, 'pure/terminal/userInputModel.js')))
  const lines = (texts) => texts.map(text => ({ text, wrapped: false }))
  for (const [harness, marker, assistant] of [['claude', '❯', '⏺'], ['claude', '>', '●'], ['codex', '›', '•']]) {
    const transcript = lines([`${marker} Привет`, '  мир', '', `${assistant} Привет мир`, '', `${marker} Привет`, '  мир'])
    assert.deepEqual(inputBlocks(transcript, [{ text: 'Привет\nмир' }], harness, 6), [{ start: 0, end: 1 }])
    assert.deepEqual(inputBlocks(lines([`${marker} hello`, '', `${assistant} hello`, `${marker} hello`, '', `${assistant} done`]),
      [{ text: 'hello' }], harness, 99), [], 'an extra role-shaped quotation makes the match ambiguous')
    assert.deepEqual(inputBlocks(lines([`${marker} hello`, '', `${assistant} hello`, `${marker} hello`, '', `${assistant} done`]),
      [{ text: 'hello' }, { text: 'hello' }], harness, 99), [{ start: 0, end: 0 }, { start: 3, end: 3 }])
    assert.deepEqual(inputBlocks(lines([`${assistant} quote hello`, `${marker} hello`, 'model continuation']), [{ text: 'hello' }], harness, 99), [])
    assert.deepEqual(inputBlocks(lines([`${marker} hello`, `${assistant} done`]), [{ text: 'hello' }], harness, 0), [], 'never decorate the composer')
    const wrapped = lines([`${marker} abc`, 'def', '', `${assistant} done`]); wrapped[1].wrapped = true
    assert.deepEqual(inputBlocks(wrapped, [{ text: 'abcdef' }], harness, 99), [{ start: 0, end: 1 }])
    assert.deepEqual(inputBlocks(lines([`${marker} [Pasted text]`, `${assistant} done`]), [{ text: 'full prompt' }], harness, 99), [])
  }

  const history = [{ordinal:1,text:'same'}, {ordinal:2,text:'unique'}, {ordinal:3,text:'same'}]
  const located = inputLocations(lines(['› same','• first','› unique','• second','› same','• third']), history, 'codex', 99)
  assert.deepEqual(located, [{start:0,end:0,ordinal:1},{start:2,end:2,ordinal:2},{start:4,end:4,ordinal:3}])
  assert.deepEqual(inputLocations(lines(['› same','• reply']), history, 'codex', 99), [], 'a clipped repeated prompt has no reliable ordinal')
  assert.deepEqual(inputLocations(lines(['› unique','• reply','› same','• last']), history, 'codex', 99),
    [{start:0,end:0,ordinal:2},{start:2,end:2,ordinal:3}], 'unique neighbours disambiguate a repeat')
  assert.equal(visibleInputOrdinal(located, 2), 1, 'the prompt at the top still belongs to the preceding output')
  assert.equal(visibleInputOrdinal(located, 3), 2, 'output below a prompt belongs to that input')
  assert.equal(visibleInputOrdinal([{start:0,end:2,ordinal:3}], 1), 2, 'multiline input must pass entirely above the viewport')
  assert.equal(visibleInputOrdinal([{start:0,end:2,ordinal:3}], 3), 3)
  assert.equal(visibleInputOrdinal([], 0), undefined, 'a clipped native grid must not guess its context')

  // Contrast is measured for the precise dedicated tokens, in each theme. Never borrow the
  // selection or search token for a speaker: the user must still be able to distinguish all three.
  const css = readFileSync(join(ui, 'src/styles/tokens.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '')
  const luminance = hex => {
    const rgb = hex.match(/\w\w/g).map(x => parseInt(x, 16) / 255).map(x => x <= .04045 ? x / 12.92 : ((x + .055) / 1.055) ** 2.4)
    return rgb[0] * .2126 + rgb[1] * .7152 + rgb[2] * .0722
  }
  for (const theme of ['light', 'dark']) {
    const body = css.slice(css.indexOf(`[data-theme='${theme}']`)).split('}')[0]
    const token = name => body.match(new RegExp(`--${name}:\\s*#([a-f0-9]{6})`, 'i'))[1]
    assert.match(body, /--user-input-bg:\s*color-mix\(in srgb, var\(--accent\) 10%, var\(--panel\)\)/)
    assert.match(body, /--user-input-edge:\s*var\(--accent\)/)
    assert.match(body, /--user-input-text:\s*var\(--text-hi\)/)
    const accent = token('accent').match(/\w\w/g).map(x => parseInt(x, 16))
    const panel = token('panel').match(/\w\w/g).map(x => parseInt(x, 16))
    const wash = accent.map((x, i) => Math.round(x * .1 + panel[i] * .9).toString(16).padStart(2, '0')).join('')
    const a = luminance(wash), b = luminance(token('text-hi'))
    assert((Math.max(a, b) + .05) / (Math.min(a, b) + .05) >= 4.5)
    assert.notEqual(wash, token('accent-dim'))
  }

  dom = new JSDOM('<!doctype html><div id="root"></div>', { url: 'https://cide.test' })
  for (const name of ['window', 'document', 'HTMLElement', 'Element', 'Node']) globalThis[name] = dom.window[name]
  globalThis.IS_REACT_ACT_ENVIRONMENT = true
  globalThis.ResizeObserver = class { observe() {} disconnect() {} }
  // jsdom has no layout engine. Supply clipped/full text heights for the mounted controls;
  // the browser audit below uses native measurements for wrapping and pane resizing.
  Object.defineProperties(dom.window.HTMLElement.prototype, {
    scrollHeight: { get() { return (this.textContent ?? '').split('\n').length * 18 } },
    clientHeight: { get() { return Math.min(2, (this.textContent ?? '').split('\n').length) * 18 } },
  })
  const pending = []
  const subscriptions = []
  globalThis.__inputFixture = {
    api: {
      userInputs: (...args) => new Promise(resolve => pending.push({ args, resolve })),
      onUserInputsChanged: fn => new Promise(resolve => subscriptions.push({ fn, resolve })),
    },
  }
  await build({ configFile: false, root: ui, logLevel: 'error',
    resolve: { alias: [
      { find: '@/ipc/client', replacement: '\0input-client' },
      { find: '@/settings/useSettings', replacement: '\0input-settings' },
      { find: '@/layout/paneHosts', replacement: '\0input-hosts' },
      { find: '@/terminal/userInputHighlight', replacement: '\0input-painter' },
      { find: '@', replacement: join(ui, 'src') },
    ] },
    plugins: [{ name: 'inputs-fixture',
      resolveId(id) { if (id.startsWith('\0input-')) return id; if (id.endsWith('virtual:inputs')) return '\0input-entry' },
      load(id) {
        if (id === '\0input-client') return 'export const sessionJournal = globalThis.__inputFixture.api;'
        if (id === '\0input-settings') return 'export const useSettings = () => null;'
        if (id === '\0input-hosts') return 'export const peekHost = () => null;'
        if (id === '\0input-painter') return 'export const highlightUserInputs = () => null;'
        if (id === '\0input-entry') return `export * from ${JSON.stringify(join(ui, 'src/panes/recapStore.ts'))}; export * from ${JSON.stringify(join(ui, 'src/panes/UserInputRecap.tsx'))};`
      },
    }],
    build: { ssr: 'virtual:inputs', outDir: join(out, 'bundle'), rollupOptions: { output: { entryFileNames: 'inputs.mjs' } } },
  })
  const m = await import(pathToFileURL(join(out, 'bundle/inputs.mjs')))
  const { createRoot } = await import('react-dom/client')
  root = createRoot(document.getElementById('root'))
  const key = m.recapKey('project', 'conversation')
  const page = (entries, total, generation = 0) => ({ inputs: entries.map(n => ({ id: String(n), ordinal: n, text: `prompt ${n}\nsecond line\nthird line` })), total, generation, available: true, hasPrevious: entries[0] > 1 })
  const resolveNext = async value => { const request = pending.shift(); assert(request); await act(async () => request.resolve(value)); return request.args }
  releases.push(m.retainRecap('project', 'conversation', false))
  await resolveNext(page([51, 52], 52))
  let unlistened = 0
  const subscription = subscriptions.shift()
  subscription.resolve(() => unlistened++)
  const revealed = []
  const navigator = {current:{pause(){},async reveal(ordinal){revealed.push(ordinal);return 'shown'}}}
  const draw = async () => act(async () => root.render(React.createElement(m.UserInputRecap, { feature: { key, project: 'project', id: 'conversation', state: m.useRecaps.getState().conversations[key],navigator } })))
  await draw()
  assert(document.body.textContent.includes('52 / 52'))
  const click = async label => act(async () => { document.querySelector(`[aria-label="${label}"]`).click() })
  await click('Show input in console')
  assert.deepEqual(revealed,[52])
  await act(async()=>document.querySelector('[aria-label="Show input in console"]').dispatchEvent(new dom.window.KeyboardEvent('keydown',{key:'Enter',bubbles:true})))
  assert.deepEqual(revealed,[52,52],'keyboard activation reveals the same input')
  const selection = window.getSelection(), range = document.createRange()
  range.selectNodeContents(document.querySelector('[aria-label="Show input in console"]'))
  selection.addRange(range)
  await click('Show input in console')
  assert.equal(revealed.length,2,'copying selected recap text must not jump the console')
  selection.removeAllRanges()
  await click('First input')
  assert.deepEqual(await resolveNext(page([1], 52)), ['project', 'conversation', 2])
  await draw()
  assert(document.body.textContent.includes('1 / 52'))
  assert(document.querySelector('[aria-label="First input"]').disabled)
  await click('Latest input'); await draw()
  assert.equal(m.useRecaps.getState().conversations[key].selected, null)
  assert(document.querySelector('[aria-label="Latest input"]').disabled)
  await click('Previous input'); await draw()
  assert.equal(m.useRecaps.getState().conversations[key].selected, 51)
  await click('Previous input')
  assert.deepEqual(await resolveNext(page([1, 2, 50], 52)), ['project', 'conversation', 51])
  await draw()
  assert(document.body.textContent.includes('50 / 52'))
  await click('Expand input'); await draw()
  assert.equal(document.querySelector('[aria-label="Collapse input"]').getAttribute('aria-expanded'), 'true')
  assert(document.body.textContent.includes('second line'))
  await click('Latest input')
  await draw()
  assert(document.body.textContent.includes('52 / 52'))
  // A new prompt can arrive while navigation waits for a historical page. Its count and
  // the selected historical prompt must survive the older response's stale total.
  m.chooseInput(key, 50); await draw()
  await click('Previous input')
  const olderRequest = pending.shift()
  assert(olderRequest)
  subscription.fn('project', 'conversation')
  await resolveNext(page([53], 53))
  assert.equal(m.useRecaps.getState().conversations[key].selected, 50)
  await act(async () => olderRequest.resolve(page([49], 52)))
  assert.equal(m.useRecaps.getState().conversations[key].total, 53)
  assert.equal(m.useRecaps.getState().conversations[key].selected, 49)
  // Replacing the transcript resets navigation. A pending previous click cannot restore
  // an ordinal from the discarded generation after its old page has been rejected.
  await draw(); await click('Previous input')
  const discardedRequest = pending.shift()
  assert(discardedRequest)
  subscription.fn('project', 'conversation')
  await resolveNext(page([1], 1, 1))
  await act(async () => discardedRequest.resolve(page([48], 52)))
  assert.equal(m.useRecaps.getState().conversations[key].selected, null)
  assert.equal(m.useRecaps.getState().conversations[key].total, 1)
  assert.deepEqual(m.useRecaps.getState().conversations[key].inputs.map(x => x.ordinal), [1])
  // Reader release invalidates an in-flight old answer, even if the same conversation opens
  // again before it arrives. A new generation also discards previously fetched older pages.
  const oldKey = m.recapKey('project', 'old')
  const stopOld = m.retainRecap('project', 'old', false)
  stopOld()
  releases.push(m.retainRecap('project', 'old', false))
  await resolveNext(page([99], 99))
  assert.equal(m.useRecaps.getState().conversations[oldKey], undefined)
  await resolveNext(page([1], 1, 1))
  assert.equal(m.useRecaps.getState().conversations[oldKey].total, 1)
  releases.forEach(stop => stop()); releases.length = 0
  assert.equal(unlistened, 1)
  console.log('user inputs: ok (history, paging, speaker boundaries, Unicode, repeats, composer, contrast, recap controls, late responses and listeners)')
} finally {
  releases.forEach(stop => stop())
  if (root) await act(async () => root.unmount())
  dom?.window.close()
  rmSync(out, { recursive: true, force: true })
}
