/** Native clipboard adaptation and React context-menu bubbling, without a Tauri host. */
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

const out = mkdtempSync(join(tmpdir(), 'cide-drawing-clipboard-'))
const windows = []
let root
try {
  execFileSync('node', [
    'node_modules/typescript/bin/tsc', 'src/panes/excalidrawClipboard.ts',
    '--outDir', out, '--module', 'esnext', '--target', 'es2022',
    '--moduleResolution', 'bundler', '--strict', '--noUncheckedIndexedAccess',
    '--exactOptionalPropertyTypes',
  ], { stdio: 'inherit' })
  const { installExcalidrawClipboard, pngPixels, claimExcalidrawContextMenu } =
    await import(`file://${join(out, 'excalidrawClipboard.js')}`)

  function hostFor(original) {
    const dom = new JSDOM('<div id="root"></div>', { url: 'https://cide.test' })
    windows.push(dom.window)
    const host = dom.window
    if (original !== undefined) Object.defineProperty(host.navigator, 'clipboard', { value: original, configurable: true })
    const urls = []
    const freed = []
    const canvases = []
    const pictures = []
    // A subarray catches accidentally sending unrelated bytes from the ImageData buffer.
    const data = new Uint8ClampedArray([99, 99, 255, 20, 10, 128, 0, 0, 0, 0, 99]).subarray(2, 10)
    const control = { decode: true, canvas: true, read: true, width: 2, height: 1 }
    host.URL.createObjectURL = (blob) => { urls.push(blob); return `blob:${urls.length}` }
    host.URL.revokeObjectURL = (url) => freed.push(url)
    const OriginalImage = host.Image
    host.Image = function () {
      const image = new OriginalImage()
      pictures.push(image)
      Object.defineProperties(image, {
        naturalWidth: { get: () => control.width },
        naturalHeight: { get: () => control.height },
        src: { set: () => queueMicrotask(() => image.dispatchEvent(new host.Event(control.decode ? 'load' : 'error'))) },
      })
      return image
    }
    const createElement = host.document.createElement.bind(host.document)
    host.document.createElement = (...args) => {
      const element = createElement(...args)
      if (args[0] === 'canvas') {
        canvases.push(element)
        element.getContext = () => control.canvas ? {
          drawImage: (image, x, y) => { assert.equal(image, pictures.at(-1)); assert.equal(x, 0); assert.equal(y, 0) },
          getImageData: () => {
            if (!control.read) throw new Error('pixel read failed')
            return { data }
          },
        } : null
      }
      return element
    }
    return { host, urls, freed, canvases, pictures, control }
  }

  const writes = []
  const backend = {
    writeText: async (text) => writes.push({ text }),
    writeImage: async ({ width, height, rgba }) => writes.push({ width, height, rgba: [...rgba] }),
  }
  const clean = hostFor()
  installExcalidrawClipboard(backend, clean.host)
  assert.ok(clean.host.navigator.clipboard.write && clean.host.ClipboardItem, 'missing browser APIs become available before capability probes')
  const svg = '<svg xmlns="http://www.w3.org/2000/svg"><text>Схема</text></svg>'
  await clean.host.navigator.clipboard.writeText(svg)
  assert.deepEqual(writes.pop(), { text: svg }, 'SVG markup goes to the native text clipboard unchanged')
  const blob = new Blob(['PNG'], { type: 'image/png' })
  for (const value of [blob, Promise.resolve(blob)]) {
    await clean.host.navigator.clipboard.write([new clean.host.ClipboardItem({ 'image/png': value })])
    assert.deepEqual(writes.pop(), { width: 2, height: 1, rgba: [255, 20, 10, 128, 0, 0, 0, 0] })
  }
  await clean.host.navigator.clipboard.write([new clean.host.ClipboardItem({ 'text/plain': Promise.resolve('text') })])
  assert.deepEqual(writes.pop(), { text: 'text' })
  assert.equal(clean.urls.length, 2)
  assert.equal(clean.freed.length, 2)
  assert.ok(clean.canvases.every((canvas) => canvas.width === 0 && canvas.height === 0), 'pixel canvases are released')
  assert.ok(clean.pictures.every((image) => image.onload === null && image.onerror === null && !image.hasAttribute('src')))
  const write = clean.host.navigator.clipboard.write
  installExcalidrawClipboard(backend, clean.host)
  assert.equal(clean.host.navigator.clipboard.write, write, 'StrictMode and another drawing do not stack wrappers')
  assert.equal(clean.host.ClipboardItem.supports('image/png'), true)
  assert.throws(() => new clean.host.ClipboardItem({}), TypeError)
  await assert.rejects(new clean.host.ClipboardItem({ 'image/png': blob }).getType('text/plain'), { name: 'NotFoundError' })
  await assert.rejects(clean.host.navigator.clipboard.write([{ types: ['text/html'] }]), { name: 'NotSupportedError' })

  // Reads and formats cide does not implement must retain their original receiver and data.
  const delegated = []
  const original = {
    read: async function () { assert.equal(this, original); return ['original image'] },
    readText: async function () { assert.equal(this, original); return 'original text' },
    write: async function (items) { assert.equal(this, original); delegated.push(items) },
  }
  const existing = hostFor(original)
  class NativeItem {}
  existing.host.ClipboardItem = NativeItem
  installExcalidrawClipboard(backend, existing.host)
  assert.equal(existing.host.navigator.clipboard, original)
  assert.equal(existing.host.ClipboardItem, NativeItem)
  const detached = hostFor()
  const detachedWrites = []
  installExcalidrawClipboard({ ...backend, writeText: async (text) => detachedWrites.push(text) }, detached.host)
  await detached.host.navigator.clipboard.writeText('another window')
  assert.deepEqual(detachedWrites, ['another window'])
  assert.equal(writes.length, 0, 'a detached window installs its own independent adapter')
  assert.deepEqual(await original.read(), ['original image'])
  assert.equal(await original.readText(), 'original text')
  for (const items of [[], [{ types: ['text/html'] }], [{ types: ['image/png', 'text/html'] }], [{ types: ['image/png'] }, { types: ['text/plain'] }]]) {
    await original.write(items)
    assert.equal(delegated.pop(), items)
  }

  // Delayed export, errors and a second installation must preserve the completion contract.
  let resolveBlob
  const pending = clean.host.navigator.clipboard.write([new clean.host.ClipboardItem({ 'image/png': new Promise((resolve) => { resolveBlob = resolve }) })])
  assert.equal(writes.length, 0, 'a promised export is awaited before native copy')
  resolveBlob(blob)
  await pending
  assert.equal(writes.length, 1)
  writes.length = 0
  const nativeError = new Error('native clipboard refused')
  installExcalidrawClipboard({ writeText: async () => { throw nativeError }, writeImage: async () => { throw nativeError } }, clean.host)
  await assert.rejects(clean.host.navigator.clipboard.writeText(svg), (error) => error === nativeError)
  await assert.rejects(clean.host.navigator.clipboard.write([new clean.host.ClipboardItem({ 'image/png': blob })]), (error) => error === nativeError)
  const exportError = new Error('export failed')
  await assert.rejects(clean.host.navigator.clipboard.write([new clean.host.ClipboardItem({ 'image/png': Promise.reject(exportError) })]), (error) => error === exportError)
  for (const [flag, reason] of [['decode', 'decoded'], ['canvas', 'canvas'], ['read', 'pixel read'], ['width', 'empty']]) {
    const bad = hostFor()
    bad.control[flag] = flag === 'width' ? 0 : false
    await assert.rejects(pngPixels(blob, bad.host), new RegExp(reason))
    assert.equal(bad.urls.length, bad.freed.length, `resources released on ${flag} failure`)
    assert.ok(bad.canvases.every((canvas) => canvas.width === 0 && canvas.height === 0))
  }

  // React bubbling matters here: Excalidraw handles the child first, then the surface shields
  // the pane frame. Capture-phase stopping would prevent the drawing's menu from opening.
  const domHost = clean.host
  for (const key of ['window', 'document', 'HTMLElement', 'Element', 'Node']) globalThis[key] = key === 'window' ? domHost : domHost[key]
  globalThis.IS_REACT_ACT_ENVIRONMENT = true
  const { createRoot } = await import('react-dom/client')
  let drawingMenus = 0
  let paneMenus = 0
  root = createRoot(document.querySelector('#root'))
  await act(async () => root.render(React.createElement('div', { onContextMenu: () => paneMenus++ },
    React.createElement('div', { id: 'surface', onContextMenu: claimExcalidrawContextMenu },
      React.createElement('canvas', { id: 'drawing', onContextMenu: (event) => { event.preventDefault(); drawingMenus++ } }),
      React.createElement('ul', { id: 'drawing-menu', onContextMenu: (event) => event.preventDefault() })),
    React.createElement('div', { id: 'footer' }))))
  for (const id of ['drawing', 'drawing-menu', 'drawing']) {
    await act(async () => document.getElementById(id).dispatchEvent(new domHost.MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2 })))
  }
  assert.equal(drawingMenus, 2)
  assert.equal(paneMenus, 0, 'canvas and drawing-menu gestures never reach the cide pane menu')
  await act(async () => document.getElementById('footer').dispatchEvent(new domHost.MouseEvent('contextmenu', { bubbles: true, cancelable: true, button: 2 })))
  assert.equal(paneMenus, 1, 'the surrounding pane chrome still owns its menu')

  const strip = (source) => source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:])\/\/.*$/gm, '$1')
  const pane = strip(readFileSync('src/panes/ExcalidrawPane.tsx', 'utf8'))
  assert.match(pane, /className=\{styles\.surface\} onContextMenu=\{claimExcalidrawContextMenu\}/)
  const installAt = pane.indexOf('installExcalidrawClipboard({')
  const importAt = pane.indexOf('void Promise.all([')
  assert.ok(installAt > 0 && importAt > installAt, 'adapter precedes cached capability probes')
  assert.match(pane, /'__TAURI_INTERNALS__' in window/, 'plain-browser demos retain their browser clipboard')
  const client = strip(readFileSync('src/ipc/client.ts', 'utf8'))
  assert.match(client, /invoke<void>\('clipboard_write_image', packFrame\(JSON\.stringify\(head\), rgba\)\)/, 'pixels use the raw IPC frame')
  console.log('excalidraw clipboard and context menu: ok')
} finally {
  if (root !== undefined) await act(async () => root.unmount())
  for (const window of windows) window.close()
  rmSync(out, { recursive: true, force: true })
}
