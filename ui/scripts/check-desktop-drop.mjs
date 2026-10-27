/** Exercise native-event routing and the real tree/tab hooks. IPC alone is substituted. */
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { resolve, join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { JSDOM } from 'jsdom'
import React, { act, StrictMode, useRef } from 'react'
import { createRoot } from 'react-dom/client'
import { build } from 'vite'

mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync(resolve('node_modules/.cache/desktop-drop-'))
const src = resolve('src')
const modules = {
  entry: `
    export * from '${src}/files/desktopDrop';
    export * from '${src}/files/dropPosition';
    export * from '${src}/sidebar/useTreeDesktopDrop';
    export * from '${src}/sidebar/useTreeDrag';
    export * from '${src}/sidebar/treeDrag';
    export * from '${src}/chrome/useTabFileDrop';
    export { control } from 'test:control';
  `,
  control: `export const control = { claims: [], copies: [], moves: [], opens: [], treeOpens: [], errors: [], exports: [], springs: [], hydrated: 0, end: null, scale: 2 };`,
  ipc: `
    import { control as c } from 'test:control';
    export const dragDrop = {
      coordinateScale: () => c.scale,
      claim: async paths => { c.claims.push([...paths]); return 'native-receipt'; },
      open: async (project, receipt, path) => { c.opens.push([project, receipt, path]); if (path.endsWith('/folder')) throw Error('Not a regular file'); },
      start: (project, paths) => { c.exports.push([project, [...paths]]); return new Promise((resolve, reject) => { c.end = error => error ? reject(error) : resolve(); }); },
    };
  `,
  notices: `import { control } from 'test:control'; export const notifyFailure = e => control.errors.push(String(e));`,
  workspace: `import { control } from 'test:control'; export const useWorkspace = { getState: () => ({ hydrate: async () => { control.hydrated++; } }) };`,
}
let root, dom
try {
  writeFileSync(join(out, 'fixture.mjs'), modules.entry)
  await build({
    configFile: false, logLevel: 'error', resolve: { alias: { '@': src } },
    plugins: [{
      name: 'desktop-drop-fixture', enforce: 'pre',
      resolveId(id) {
        if (id.startsWith('test:')) return `\0${id}`
        if (id === '@/ipc/client' || id === `${src}/ipc/client`) return '\0test:ipc'
        if (id === '@/store/workspace' || id === `${src}/store/workspace`) return '\0test:workspace'
        if (id === '@/chrome/notices' || id === `${src}/chrome/notices` || id === './notices') return '\0test:notices'
      },
      load(id) { if (id.startsWith('\0test:')) return modules[id.slice('\0test:'.length)] },
    }],
    build: { ssr: join(out, 'fixture.mjs'), outDir: out, emptyOutDir: false,
      rollupOptions: { external: ['react'], output: { entryFileNames: 'entry.mjs' } } },
  })
  dom = new JSDOM('<div id="root"></div>', { url: 'https://cide.test' })
  for (const key of ['window', 'document', 'HTMLElement', 'Element', 'Node']) globalThis[key] = key === 'window' ? dom.window : dom.window[key]
  globalThis.IS_REACT_ACT_ENVIRONMENT = true
  window.HTMLElement.prototype.hasPointerCapture = () => false
  window.HTMLElement.prototype.setPointerCapture = () => {}
  Object.defineProperty(window, 'devicePixelRatio', { value: 2 })
  const { routeDesktopDrop, exportTreeDrag, useTreeDesktopDrop, useTreeDrag, useTabFileDrop, externalDropOutcome, dropCoordinateScale, control: c } = await import(pathToFileURL(join(out, 'entry.mjs')).href)
  assert.equal(dropCoordinateScale('Macintosh; Intel Mac OS X', 2), 1, 'Retina Cocoa points are already logical')
  assert.equal(dropCoordinateScale('Windows', 2), 2)
  assert.equal(dropCoordinateScale('Linux', 1.5), 1.5)
  const rows = [
    { path: '/p', kind: 'dir', name: 'p', expanded: true, hasChildren: true },
    { path: '/p/dest', kind: 'dir', name: 'dest', expanded: false, hasChildren: true },
    { path: '/p/dest/file.txt', kind: 'file', name: 'file.txt', expanded: false, hasChildren: false },
    { path: '/p/source.txt', kind: 'file', name: 'source.txt', expanded: false, hasChildren: false },
  ]
  // Explorer normally hides the project-root row.
  const rowFor = path => path === '/p' ? null : rows.find(row => row.path === path) ?? null
  let under = null, state = null, hot = false
  document.elementFromPoint = (x, y) => { assert.equal(x, 50); assert.equal(y, 50); return under }
  function Fixture() {
    const tree = useRef(null), strip = useRef(null)
    const options = { container: tree, roots: ['/p'], writable: ['/p'], rowFor,
      onSpring: path => c.springs.push(path), onMove: (...args) => c.moves.push(args),
      onCopy: (...args) => c.copies.push(args), onError: error => c.errors.push(String(error)),
    }
    state = useTreeDesktopDrop(options)
    hot = useTabFileDrop(strip, 'project')
    const pointer = useTreeDrag({ ...options, carried: new Set(['/p/source.txt']),
      onOpen: paths => c.treeOpens.push([...paths]),
      onExport: load => void exportTreeDrag('project', load) })
    return React.createElement('main', {},
      React.createElement('div', { id: 'tree', ref: tree }, rows.map(row => React.createElement('div', {
        key: row.path, 'data-row-path': row.path,
        onPointerDown: event => pointer.onPointerDown(event, row),
      }, row.name))),
      React.createElement('div', { id: 'tabs', ref: strip, 'data-file-drop-project': 'project' }, React.createElement('span', { id: 'tab' }, 'tab')),
      React.createElement('div', { id: 'overlay' }),
    )
  }
  root = createRoot(document.getElementById('root'))
  await act(async () => root.render(React.createElement(StrictMode, {}, React.createElement(Fixture))))
  const event = (type, paths = ['/outside/你好 a.txt']) => ({ type, paths, position: { x: 100, y: 100 } })
  const send = async (type, paths) => act(async () => routeDesktopDrop(event(type, paths)))
  under = document.querySelector('[data-row-path="/p/dest/file.txt"]')
  await send('enter')
  assert.equal(state.outcome.mark, '/p/dest', 'a file row highlights its parent folder')
  assert.match(state.outcome.hint, /^Copy /)
  assert.equal(c.copies.length, 0, 'hover never writes')
  await send('over')
  assert.equal(state.drag.paths[0], '/outside/你好 a.txt', 'over retains enter paths')
  await send('drop')
  assert.deepEqual(c.copies, [[['/outside/你好 a.txt'], '/p/dest', 'native-receipt']])
  assert.equal(state, null)
  under = document.getElementById('tree')
  await send('enter'); assert.equal(state.outcome.mark, '/p', 'blank space inside tree means root')
  await act(async () => routeDesktopDrop({ type: 'leave' }))
  assert.equal(state, null)
  c.scale = 1
  await act(async () => routeDesktopDrop({ type: 'enter', paths: ['/outside/mac.txt'], position: { x: 50, y: 50 } }))
  assert.equal(state.x, 50, 'Retina points reach the same CSS target without another division')
  await act(async () => routeDesktopDrop({ type: 'leave' }))
  c.scale = 2
  under = document.getElementById('overlay')
  await send('drop'); assert.equal(c.copies.length, 1, 'a modal or another surface cannot import')
  under = document.getElementById('tab')
  await send('enter'); assert.equal(hot, true)
  await send('drop', ['/outside/a.txt', '/outside/folder', '/outside/b.txt'])
  assert.equal(hot, false)
  assert.deepEqual(c.opens.map(call => call[2]), ['/outside/a.txt', '/outside/folder', '/outside/b.txt'])
  assert.equal(c.hydrated, 1)
  assert.equal(c.errors.length, 1, 'invalid dropped entries report errors without losing other files')
  assert.equal(c.copies.length, 1, 'dropping on tabs only opens originals')

  const load = { paths: ['/p/source.txt'], origin: '/p/source.txt', label: 'source.txt', widened: false, refusal: null }
  let exported
  await act(async () => { exported = exportTreeDrag('project', load) })
  under = document.querySelector('[data-row-path="/p/dest"]')
  await send('enter', load.paths)
  assert.match(state.outcome.hint, /^Move /)
  await send('drop', load.paths)
  assert.deepEqual(c.moves, [[['/p/source.txt'], '/p/dest']], 'reentering an exported tree gesture still moves')
  await act(async () => { c.end(); await exported })

  under = document.querySelector('[data-row-path="/p/source.txt"]')
  const pointerEvent = (type, x) => {
    const e = new window.MouseEvent(type, { bubbles: true, button: 0, clientX: x, clientY: 50 })
    Object.defineProperty(e, 'pointerId', { value: 1 })
    return e
  }
  await act(async () => under.dispatchEvent(pointerEvent('pointerdown', 40)))
  under = document.getElementById('tab')
  await act(async () => window.dispatchEvent(pointerEvent('pointermove', 50)))
  await act(async () => window.dispatchEvent(pointerEvent('pointerup', 50)))
  assert.deepEqual(c.treeOpens, [['/p/source.txt']], 'tree to tabs opens directly without starting a desktop session')
  assert.equal(c.exports.length, 1)
  under = document.querySelector('[data-row-path="/p/source.txt"]')
  await act(async () => under.dispatchEvent(pointerEvent('pointerdown', 40)))
  under = document.getElementById('overlay')
  await act(async () => window.dispatchEvent(pointerEvent('pointermove', 50)))
  assert.equal(c.exports.length, 2, 'a pointer drag leaving the tree starts a native export')
  await act(async () => window.dispatchEvent(pointerEvent('pointerup', 50)))
  assert.equal(c.moves.length, 1, 'the old pointer gesture never also commits a move')
  await act(async () => c.end(Error('Native drag failed')))
  assert.match(c.errors.at(-1), /Native drag failed/)
  under = document.querySelector('[data-row-path="/p/dest"]')
  await send('enter')
  assert.match(state.outcome.hint, /^Copy /, 'completion clears native source provenance')

  assert.equal(externalDropOutcome(['/p/dest/file.txt'], rows[1], ['/p'], ['/p']).kind, 'move', 'copying beside itself goes through collision handling')
  assert.equal(externalDropOutcome(['/outside'], rows[1], ['/p'], ['/p']).kind, 'move')
  assert.equal(externalDropOutcome(['/p/dest'], rows[1], ['/p'], ['/p']).kind, 'refuse')
  assert.equal(externalDropOutcome(['/outside/a'], { ...rows[1], path: '/read-only' }, ['/p'], ['/p']).kind, 'refuse')
  await act(async () => root.unmount()); root = null
  await send('drop'); assert.equal(c.copies.length, 1, 'unmount unregisters drop receivers under StrictMode')
  console.log('desktop drops: native routing, tree copy/move, tab opening, export handoff and cleanup: ok')
} finally {
  if (root) await act(async () => root.unmount())
  dom?.window.close()
  rmSync(out, { recursive: true, force: true })
}
