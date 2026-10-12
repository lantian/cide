/** Exercise real gallery input, focus and camera behavior without a browser dependency. */
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { resolve } from 'node:path'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync('node_modules/.cache/cide-image-viewer-')
const dom = new JSDOM('<button id="opener">Open image</button><div id="root"></div>', { url: 'https://cide.test', pretendToBeVisual: true })
for (const key of ['window', 'document', 'HTMLElement', 'Element', 'Node']) globalThis[key] = dom.window[key]
globalThis.IS_REACT_ACT_ENVIRONMENT = true
const { createRoot } = await import('react-dom/client')
let root
try {
  execFileSync('node', ['node_modules/vite/bin/vite.js', 'build', '--ssr', 'src/overlays/imageViewerSmoke.ts', '--outDir', out, '--logLevel', 'error'], { stdio: 'inherit' })
  const { ImageViewer, AttachmentLightbox, fitImage, constrainImage, zoomImage } = await import(`file://${resolve(out, 'imageViewerSmoke.js')}`)
  const imageSize = { width: 1600, height: 1200 }
  const viewportSize = { width: 800, height: 600 }
  assert.equal(fitImage(imageSize, viewportSize), 0.5)
  assert.equal(fitImage({ width: 200, height: 300 }, viewportSize), 1, 'fit keeps small images at natural size')
  assert.equal(fitImage({ width: 1000, height: 30000 }, viewportSize), 0.02, 'very tall images fit below 10%')
  assert.equal(constrainImage({ scale: 0.001, x: 0, y: 0 }, { width: 1000, height: 30000 }, viewportSize).scale, 0.02)
  assert.deepEqual(constrainImage({ scale: 1, x: 9999, y: -9999 }, imageSize, viewportSize), { scale: 1, x: 400, y: -300 })
  const original = { scale: 1, x: 60, y: -40 }
  const point = { x: 100, y: 70 }
  const enlarged = zoomImage(original, 2, point, imageSize, viewportSize)
  assert.equal((point.x - original.x) / original.scale, (point.x - enlarged.x) / enlarged.scale, 'zoom holds the pixel under the pointer horizontally')
  assert.equal((point.y - original.y) / original.scale, (point.y - enlarged.y) / enlarged.scale, 'zoom holds the pixel under the pointer vertically')

  let area = viewportSize
  Object.defineProperties(dom.window.HTMLElement.prototype, {
    clientWidth: { configurable: true, get: () => area.width },
    clientHeight: { configurable: true, get: () => area.height },
  })
  Object.defineProperties(dom.window.HTMLImageElement.prototype, {
    naturalWidth: { configurable: true, get: () => imageSize.width },
    naturalHeight: { configurable: true, get: () => imageSize.height },
  })
  const captures = new Set()
  dom.window.HTMLElement.prototype.setPointerCapture = (id) => captures.add(id)
  dom.window.HTMLElement.prototype.releasePointerCapture = (id) => captures.delete(id)
  const images = [
    { id: 'a', title: 'First', previewId: 'shared' },
    { id: 'b', title: 'Second', previewId: 'shared' },
    { id: 'c', title: 'Third', previewId: 'c' },
  ]
  const previews = { shared: { kind: 'ready', url: 'https://cide.test/shared.png' } }
  const requests = []
  const opens = []
  let closed = 0
  let parentKeys = 0
  const opener = document.querySelector('#opener')
  root = createRoot(document.querySelector('#root'))
  function Gallery({ entries = images, initial = 'b', previewMap = previews }) {
    const [current, setCurrent] = React.useState(initial)
    const [open, setOpen] = React.useState(true)
    return React.createElement('div', { onKeyDown: () => parentKeys++ }, open && React.createElement(ImageViewer, {
      images: entries, current, previews: previewMap, returnFocus: opener,
      onStep: setCurrent, onClose: () => { closed++; setOpen(false) },
      onRequestPreview: (id) => requests.push(id), onOpen: (id) => opens.push(id),
    }))
  }
  const mount = async (props = {}, key = 'gallery') => act(async () => root.render(React.createElement(Gallery, { key, ...props })))
  const viewer = () => document.querySelector('[data-audit="imageViewer"]')
  const stage = () => document.querySelector('[data-audit="imageViewerViewport"]')
  const picture = () => viewer().querySelector('img')
  const button = (label) => [...viewer().querySelectorAll('button')].find((element) => element.getAttribute('aria-label') === label || element.textContent === label)
  const click = async (label) => act(async () => { const element = button(label); assert.ok(element, label); element.click() })
  const key = async (value, shiftKey = false) => act(async () => document.activeElement.dispatchEvent(new dom.window.KeyboardEvent('keydown', { key: value, shiftKey, bubbles: true, cancelable: true })))
  const pointer = async (type, x, y) => act(async () => {
    const event = new dom.window.MouseEvent(type, { bubbles: true, clientX: x, clientY: y, button: 0 })
    Object.defineProperty(event, 'pointerId', { value: 1 })
    stage().dispatchEvent(event)
  })

  opener.focus()
  await mount()
  assert.equal(viewer().getAttribute('aria-label'), 'Second')
  assert.ok(viewer().textContent.includes('2 / 3'))
  assert.equal(document.activeElement, button('Close'))
  assert.equal(document.querySelector('[data-audit="imageViewerZoom"]').textContent, '50%')
  await click('Zoom in')
  assert.equal(picture().style.width, '1000px')
  await click('100%')
  assert.equal(picture().style.width, '1600px')
  await pointer('pointerdown', 300, 200)
  assert.ok(captures.has(1))
  await pointer('pointermove', 400, 250)
  assert.ok(picture().style.transform.includes('translate(100px, 50px)'))
  await pointer('pointermove', 10000, 10000)
  assert.ok(picture().style.transform.includes('translate(400px, 300px)'))
  await pointer('pointerup', 10000, 10000)
  assert.ok(!captures.has(1))
  await click('Fit')
  assert.equal(picture().style.width, '800px')
  assert.ok(picture().style.transform.includes('translate(0px, 0px)'))

  let wheel
  await act(async () => {
    wheel = new dom.window.WheelEvent('wheel', { deltaY: -100, clientX: 400, clientY: 300, bubbles: true, cancelable: true })
    stage().dispatchEvent(wheel)
  })
  assert.ok(wheel.defaultPrevented, 'wheel zoom does not scroll the page')
  assert.ok(parseFloat(picture().style.width) > 800)
  await click('100%')
  area = { width: 2000, height: 1600 }
  await act(async () => window.dispatchEvent(new dom.window.Event('resize')))
  assert.equal(stage().dataset.pan, 'false', 'resize recenters an image that now fits')
  area = viewportSize
  await act(async () => window.dispatchEvent(new dom.window.Event('resize')))
  await click('Zoom in')
  for (let i = 0; i < 20; i++) await click('Zoom in')
  assert.ok(button('Zoom in').disabled)
  assert.equal(document.querySelector('[data-audit="imageViewerZoom"]').textContent, '1600%')
  for (let i = 0; i < 30; i++) await click('Zoom out')
  assert.ok(button('Zoom out').disabled)
  assert.equal(document.querySelector('[data-audit="imageViewerZoom"]').textContent, '10%')

  await click('Previous image')
  assert.equal(viewer().getAttribute('aria-label'), 'First', 'shared URL preserves distinct gallery entries')
  assert.ok(button('Previous image').disabled)
  assert.equal(document.querySelector('[data-audit="imageViewerZoom"]').textContent, '50%', 'stepping resets the camera even for the same URL')
  await key('ArrowLeft')
  assert.equal(viewer().getAttribute('aria-label'), 'First')
  await key('ArrowRight')
  await key('ArrowRight')
  assert.equal(viewer().getAttribute('aria-label'), 'Third')
  assert.ok(button('Next image').disabled)
  assert.ok(viewer().textContent.includes('Loading image'))
  assert.ok(requests.includes('c'))
  await mount({ previewMap: { ...previews, c: { kind: 'refused', reason: 'Access refused' } } })
  assert.ok(viewer().textContent.includes('Access refused'))
  await click('Previous image')
  await click('Open externally')
  assert.deepEqual(opens, ['b'])
  await act(async () => opener.focus())
  assert.equal(document.activeElement, button('Close'), 'background controls cannot take focus')
  const controls = [...viewer().querySelectorAll('button:not(:disabled)')]
  await act(async () => controls.at(-1).focus())
  await key('Tab')
  assert.equal(document.activeElement, controls[0])
  await key('Tab', true)
  assert.equal(document.activeElement, controls.at(-1))
  await key('Escape')
  assert.equal(viewer(), null)
  assert.equal(document.activeElement, opener)
  assert.equal(parentKeys, 0, 'gallery keystrokes do not reach the parent dialog')

  await mount({ entries: [images[0]], initial: 'a' }, 'single')
  assert.equal(button('Next image'), undefined)
  assert.equal(button('Previous image'), undefined)
  await act(async () => picture().dispatchEvent(new dom.window.Event('error')))
  assert.ok(viewer().textContent.includes('Image unavailable'))
  assert.ok(button('Zoom in').disabled)
  await mount({ entries: [], initial: 'a' }, 'single')
  assert.equal(viewer(), null, 'removing the current image closes the viewer')
  assert.equal(closed, 2)

  await act(async () => root.render(React.createElement(AttachmentLightbox, {
    images: [{ id: 'a', name: 'Attachment.png', bytes: 2048, kind: 'image' }], current: 'a',
    previews: { a: previews.shared }, onClose() {}, onStep() {}, onOpen: (id) => opens.push(id),
  })))
  const attachment = document.querySelector('[data-audit="attachmentLightbox"]')
  assert.ok(attachment.textContent.includes('2 KiB'))
  assert.ok(attachment.querySelector('[data-audit="imageViewerZoom"]'))
  await act(async () => attachment.querySelector('[data-audit="attachmentLightboxOpen"]').click())
  assert.equal(opens.at(-1), 'a')
  await act(async () => root.render(null))
  opener.focus()
  function StrictAttachments() {
    const [open, setOpen] = React.useState(true)
    return open && React.createElement(AttachmentLightbox, {
      images: [{ id: 'a', name: 'Attachment.png', bytes: 2048, kind: 'image' }], current: 'a',
      previews: { a: previews.shared }, onClose: () => setOpen(false), onStep() {},
    })
  }
  await act(async () => root.render(React.createElement(React.StrictMode, null, React.createElement(StrictAttachments))))
  await key('Escape')
  assert.equal(document.querySelector('[data-audit="attachmentLightbox"]'), null)
  assert.equal(document.activeElement, opener, 'StrictMode retains the original attachment opener through effect replay')
  console.log('check-image-viewer: ok (gallery order, shared previews, loading/errors, fit/zoom limits, wheel, pointer capture/panning, resize, focus, Escape and attachment adapter)')
} finally {
  if (root) await act(async () => root.unmount())
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
