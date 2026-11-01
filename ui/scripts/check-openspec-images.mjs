import { strict as assert } from 'node:assert'
import { mkdtempSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'
const ui = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(ui, 'node_modules/.cache/spec-images-'))
const dom = new JSDOM('<div id="root"></div>', { url: 'https://cide.test' })
for (const key of ['window', 'document', 'HTMLElement', 'Element', 'Node']) globalThis[key] = dom.window[key]
globalThis.IS_REACT_ACT_ENVIRONMENT = true
const { createRoot } = await import('react-dom/client')
let seq = 0, delayed, fail = false
const released = [], errors = []
window.imagesTest = {
  stage: async () => { if (fail) throw Error('bad image'); if (delayed) await delayed; return [{ path: `/draft/${++seq}.png` }] },
  release: async paths => released.push(...paths), pick: async () => [],
  error: e => errors.push(e),
}
let root
try {
  await build({ root: ui, configFile: false, logLevel: 'error',
    resolve: { alias: { '@': join(ui, 'src') } },
    plugins: [{ name: 'image-test-ipc', enforce: 'pre',
      resolveId(s) { if ((s === '@/ipc/client' || s === join(ui, 'src/ipc/client'))) return '\0ipc'; if ((s === '@/chrome/notices' || s === join(ui, 'src/chrome/notices'))) return '\0notices' },
      load(id) {
        if (id === '\0ipc') return 'export const specImages = window.imagesTest; export const image = {srcUrl: d => d.path};'
        if (id === '\0notices') return 'export const notifyFailure = window.imagesTest.error;'
      },
    }],
    build: { ssr: join(ui, 'src/sidebar/OpenSpecPanel/useImages.ts'), outDir: out, rollupOptions: { output: { entryFileNames: 'images.mjs' } } },
  })
  const { useImages } = await import(pathToFileURL(join(out, 'images.mjs')))
  let current
  function Probe({ scope }) { current = useImages(scope); return null }
  root = createRoot(document.getElementById('root'))
  const render = async scope => { await act(async () => root.render(React.createElement(Probe, {scope}))) }
  await render('project:propose')
  await act(async () => current.paste())
  assert.equal(current.docs.length, 1)
  await act(async () => current.drop(['/original/screen shot.png']))
  assert.equal(current.docs.length, 2)
  assert.equal(current.previews[1].name, 'screen shot.png')
  await act(async () => current.remove('/draft/1.png'))
  assert.deepEqual(released, ['/draft/1.png'])
  fail = true
  await act(async () => current.paste())
  assert.equal(current.docs.length, 1, 'failed paste keeps existing images')
  assert.equal(errors.length, 1)
  fail = false
  let release
  delayed = new Promise(r => { release = r })
  await act(async () => current.paste())
  assert.equal(current.pending, true)
  await render('project:explore')
  assert.equal(current.docs.length, 0)
  await act(async () => release())
  assert.equal(current.docs.length, 0, 'late paste cannot enter another workflow')
  assert.deepEqual(released, ['/draft/1.png', '/draft/2.png', '/draft/3.png'])
  delayed = null
  await act(async () => current.pick())
  assert.equal(current.docs.length, 0, 'cancelled picker adds nothing')
  assert.equal(current.pending, false)
  console.log('OpenSpec images: staging, removal, failures, cancellation and stale completions passed')
} finally {
  if (root) await act(async () => root.unmount())
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
