import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { resolve } from 'node:path'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync('node_modules/.cache/cide-image-diff-')
const dom = new JSDOM('<!doctype html><div id="root"></div>', {
  url: 'https://cide.test', pretendToBeVisual: true,
})
for (const name of ['window', 'document', 'location', 'HTMLElement', 'Element', 'Node', 'MutationObserver']) {
  globalThis[name] = dom.window[name]
}
globalThis.self = dom.window
globalThis.IS_REACT_ACT_ENVIRONMENT = true
globalThis.requestAnimationFrame = (fn) => setTimeout(fn, 0)
globalThis.cancelAnimationFrame = clearTimeout
globalThis.ResizeObserver = class { observe() {} disconnect() {} unobserve() {} }
dom.window.HTMLElement.prototype.scrollIntoView = () => {}
window.__TAURI_INTERNALS__ = {
  convertFileSrc: (path) => `https://asset.localhost/${encodeURIComponent(path)}`,
}
const { createRoot } = await import('react-dom/client')
const root = createRoot(document.getElementById('root'))
const container = document.getElementById('root')
const ready = (path, format = 'png') => ({ kind: 'ready', doc: { path, format, bytes: 1234, stamp: null } })
const absent = { kind: 'absent' }
const diff = (old, next, extra = {}) => ({
  path: 'picture.png', oldPath: null, side: 'unstaged', status: 'modified',
  binary: true, oldMode: 33188, newMode: 33188, rev: 'rev', partialOk: false,
  oldText: null, newText: null, textsOmitted: false, hunks: [],
  images: { oldPath: null, old, new: next }, ...extra,
})
const common = {
  path: 'picture.png', collapsed: new Set(), busy: false, note: null, reason: null,
  onCollapse() {}, onCurrentChange() {}, onView() {}, onBlame() {},
}
let GitDiffView
async function render(value, readOnly = false, extra = {}) {
  const props = readOnly
    ? { ...common, readOnly: true, diff: value, revisions: { from: 'aaa1111', to: 'bbb2222' }, ...extra }
    : { ...common, diff: value, side: 'unstaged', marks: new Set(), held: null,
        onSide() {}, onMarks() {}, onApply() {}, onDropHeld() {}, ...extra }
  await act(async () => root.render(React.createElement(GitDiffView, props)))
}
async function click(el) {
  assert.ok(el)
  await act(async () => el.dispatchEvent(new dom.window.MouseEvent('click', { bubbles: true })))
}
function side(position) { return container.querySelector(`[data-audit="gitImageSide"][data-side="${position}"]`) }
function button(column, text) { return [...column.querySelectorAll('button')].find((el) => el.textContent === text) }

try {
  execFileSync('node', ['node_modules/vite/bin/vite.js', 'build', '--ssr',
    'src/panes/imageDiffSmoke.tsx', '--outDir', out, '--logLevel', 'error'], { stdio: 'inherit' })
  const module = await import(`file://${resolve(out, 'imageDiffSmoke.js')}`)
  GitDiffView = module.GitDiffView
  assert.deepEqual(module.imageSideLabels('staged'), { old: 'HEAD', new: 'Index' })
  assert.deepEqual(module.imageSideLabels('combined'), { old: 'HEAD', new: 'Working tree' })
  const pair = diff(ready('/snapshots/old.png'), ready('/snapshots/new.png'))
  await render(pair)
  assert.equal(module.focusedChangeNav(), null, 'Images must not claim text change navigation')
  assert.equal(container.querySelectorAll('img').length, 2)
  assert.match(side('old').textContent, /Before · Index/)
  assert.match(side('new').textContent, /After · Working tree/)
  assert.equal(container.querySelector('[data-audit="gitDiffBlameToggle"]'), null)
  assert.equal(container.querySelector('[data-audit="gitDiffStep"]'), null)
  assert.equal(container.querySelector('[data-audit="gitDiffApply"]'), null)
  assert.equal(container.querySelector('[data-audit="gitDiffRow"]'), null)
  assert.equal(button(side('old'), 'Fit').getAttribute('aria-pressed'), 'true')
  await click(button(side('old'), '1:1'))
  assert.equal(button(side('old'), '1:1').getAttribute('aria-pressed'), 'true')
  assert.equal(button(side('new'), 'Fit').getAttribute('aria-pressed'), 'true')
  await click(button(side('old'), 'Fit'))
  const img = side('old').querySelector('img')
  Object.defineProperty(img, 'naturalWidth', { value: 2380 })
  Object.defineProperty(img, 'naturalHeight', { value: 592 })
  await act(async () => img.dispatchEvent(new dom.window.Event('load')))
  assert.match(side('old').textContent, /PNG · 2380 × 592 · 1 KiB/)
  assert.doesNotMatch(side('old').textContent, /Loading image/)

  await render(pair, true)
  assert.match(side('old').textContent, /Before · aaa1111/)
  assert.match(side('new').textContent, /After · bbb2222/)
  assert.equal(container.querySelectorAll('img').length, 2)
  await render(pair, false, { side: 'staged' })
  assert.match(side('old').textContent, /Before · HEAD/)
  assert.match(side('new').textContent, /After · Index/)
  await render(pair, false, { side: 'combined' })
  assert.match(side('new').textContent, /After · Working tree/)

  await render(diff(absent, ready('/snapshots/added.png'), { status: 'added' }))
  assert.equal(side('old'), null)
  assert.equal(container.querySelectorAll('img').length, 1)
  assert.equal(container.querySelector('[data-audit="gitImageDiff"]').dataset.single, 'true')
  await render(diff(ready('/snapshots/deleted.png'), absent, { status: 'deleted' }), true)
  assert.equal(side('new'), null)
  assert.equal(container.querySelectorAll('img').length, 1)

  const svg = diff(ready('/snapshots/old.svg', 'svg'), ready('/snapshots/new.svg', 'svg'), {
    path: 'after.svg', binary: false, partialOk: true,
    oldText: '<svg/>', newText: '<svg width="2"/>',
    images: { oldPath: 'before.svg', old: ready('/snapshots/old.svg', 'svg'), new: ready('/snapshots/new.svg', 'svg') },
  })
  await render(svg)
  assert.equal(module.focusedChangeNav(), null)
  assert.match(side('old').textContent, /before.svg/)
  assert.match(side('new').textContent, /after.svg/)
  assert.equal(container.querySelectorAll('svg script').length, 0)
  assert.equal(container.querySelectorAll('img').length, 2)
  assert.equal(container.querySelector('[data-audit="gitDiffApply"]'), null)

  await render(diff({ kind: 'unavailable', reason: 'Image exceeds 32 MiB' }, ready('/snapshots/valid.png')))
  assert.match(side('old').textContent, /32 MiB/)
  assert.equal(side('new').querySelectorAll('img').length, 1)
  await act(async () => side('new').querySelector('img').dispatchEvent(new dom.window.Event('error')))
  assert.match(side('new').textContent, /could not be decoded/)
  await click(button(side('new'), 'Try again'))
  assert.match(side('new').querySelector('img').src, /retry=1/)

  await render(diff(ready('/snapshots/valid.png'), ready('/snapshots/replaced.png')))
  assert.match(side('new').querySelector('img').src, /replaced.png/)
  assert.equal(side('new').querySelector('button[aria-pressed="true"]').textContent, 'Fit')
  await render(pair, false, { visible: false })
  assert.equal(container.querySelectorAll('img').length, 0)
  assert.ok(container.querySelector('[data-audit="gitDiffParked"]'))
  const textDiff = { ...pair, images: undefined, path: 'source.txt', binary: false, partialOk: true }
  await render(textDiff)
  assert.ok(module.focusedChangeNav(), 'Text diffs still claim change navigation')
  await render(pair)
  assert.equal(module.focusedChangeNav(), null, 'A text-to-image refresh releases the navigation claim')
  console.log('Image diffs: Changes/Log, revisions, one-sided files, SVG, metadata, zoom, errors and refresh passed')
} finally {
  await act(async () => root.unmount())
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
