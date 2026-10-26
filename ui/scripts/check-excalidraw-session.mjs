/**
 * Exercise the real drawing host, pane, workspace reconciliation and save registry under
 * React/jsdom. Only IPC and the canvas engine are substituted. The engine keeps private
 * history so remounting it cannot accidentally pass a snapshot-only preservation test.
 * Actual canvas layout/rendering still needs a display.
 */
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { resolve, join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { JSDOM } from 'jsdom'
import React, { act, StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { build } from 'vite'

mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync(resolve('node_modules/.cache/drawing-session-'))
const src = resolve('src')
const modules = {
  entry: `
    export { ExcalidrawHost, drawingSessions } from '${src}/panes/ExcalidrawHost';
    export { createDrawingHosts } from '${src}/panes/drawingHosts';
    export { saveTab, registeredBuffers } from '${src}/editor/openBuffers';
    export { readoutClaims } from '${src}/editor/statusReadout';
    export { useWorkspace } from 'test:workspace';
    export { control } from 'test:control';
  `,
  control: `
    export const control = {
      records: new Map(), canvases: new Map(), tool: new Set(), git: new Set(), history: new Set(), historyMounts: new Map(), acks: [], nextMount: 0,
      dirty: [], reads: [], writes: [], mounts: 0, unmounts: 0, nextId: 0, nextStat: null, refreshes: [],
    };
  `,
  workspace: `
    import { create } from 'zustand';
    export const useWorkspace = create(() => ({ boot: null, theme: 'dark' }));
  `,
  ipc: `
    import { control } from 'test:control';
    export const clipboard = {}, imageClipboard = {};
    export const diag = { log: async () => {} };
    export const fileChanged = error => error?.changed === true;
    const listen = set => async handler => { set.add(handler); return () => set.delete(handler); };
    export const events = { onSessionTool: listen(control.tool), onGitStatus: listen(control.git), onLocalHistoryEditorGate: listen(control.history) };
    export const localHistory = {
      registerEditor: async (editor, canonicalPath) => {
        const registration = { window: 'test', editor, canonicalPath, mount: String(++control.nextMount) };
        control.historyMounts.set(editor, registration); return registration;
      },
      unregisterEditor: async (editor, mount) => { if (control.historyMounts.get(editor)?.mount === mount) control.historyMounts.delete(editor); },
      acknowledge: async (editor, mount, request, dirty) => { control.acks.push({ editor, mount, request, dirty }); },
    };
    export const file = {
      readBytes: async path => {
        control.reads.push(path);
        const record = control.records.get(path);
        return { head: { stamp: { ...record.stamp }, writable: true }, bytes: record.bytes.slice() };
      },
      stat: async path => {
        if (control.nextStat) { const pending = control.nextStat; control.nextStat = null; return pending; }
        return { ...control.records.get(path).stamp };
      },
      setDirty: async (project, tab, dirty) => { control.dirty.push({ project, tab, dirty }); },
      writeBytes: async (path, bytes, expected) => {
        const record = control.records.get(path);
        if (expected && expected.mtimeNanos !== record.stamp.mtimeNanos) throw { changed: true };
        const stamp = { mtimeNanos: String(Number(record.stamp.mtimeNanos) + 1), len: bytes.length };
        control.records.set(path, { stamp, bytes: bytes.slice() });
        control.writes.push(path);
        return stamp;
      },
    };
  `,
  surface: `
    import React, { useLayoutEffect, useRef } from 'react';
    import { control } from 'test:control';
    import { sceneFingerprint } from '${src}/panes/excalidrawKinds';
    export const liveElementIds = elements => elements.filter(e => !e.isDeleted).map(e => e.id).sort().join('\\n');
    export const fingerprintOfChange = (elements, state, files) =>
      sceneFingerprint(elements.reduce((sum, e) => sum + e.version, 0), Object.keys(files), state);
    export const fingerprintOf = api => fingerprintOfChange(api.getSceneElementsIncludingDeleted(), api.getAppState(), api.getFiles());
    export const captureViewport = api => {
      const { scrollX, scrollY, zoom } = api.getAppState(); return { scrollX, scrollY, zoom };
    };
    export const refreshSurface = api => { control.refreshes.push(api.id); };
    export const loadScene = async (bytes, _kind, viewport) => {
      const scene = JSON.parse(new TextDecoder().decode(bytes));
      return { ...scene, appState: { ...scene.appState, ...viewport } };
    };
    export const serializeScene = async api => new TextEncoder().encode(JSON.stringify({
      elements: api.getSceneElements(), files: api.getFiles(), appState: api.getAppState(),
    }));
    export function Surface({ initial, onApi, onChange }) {
      const latest = useRef(onChange); latest.current = onChange;
      const ref = useRef(null);
      if (!ref.current) {
        let elements = structuredClone(initial.elements), files = structuredClone(initial.files);
        const state = { scrollX: 0, scrollY: 0, zoom: { value: 1 }, ...initial.appState };
        const undo = [], redo = [];
        const emit = () => latest.current(elements, state, files);
        ref.current = {
          id: String(++control.nextId),
          getAppState: () => state, getFiles: () => files,
          getSceneElements: () => elements.filter(e => !e.isDeleted),
          getSceneElementsIncludingDeleted: () => elements,
          updateScene: ({ appState }) => { Object.assign(state, appState); emit(); },
          view: patch => { Object.assign(state, patch); emit(); },
          edit: () => {
            undo.push(structuredClone(elements)); redo.length = 0;
            elements = elements.map(e => ({ ...e, version: e.version + 1 })); emit();
          },
          undo: () => { if (undo.length) { redo.push(elements); elements = undo.pop(); emit(); } },
          redo: () => { if (redo.length) { undo.push(elements); elements = redo.pop(); emit(); } },
        };
      }
      const api = ref.current;
      useLayoutEffect(() => {
        control.mounts++; control.canvases.set(api.id, api); onApi(api);
        latest.current(api.getSceneElementsIncludingDeleted(), api.getAppState(), api.getFiles());
        return () => { control.unmounts++; control.canvases.delete(api.id); };
      }, []);
      return React.createElement('div', { 'data-canvas': api.id, onContextMenu: e => e.preventDefault() }, 'Drawing');
    }
  `,
}

let outer
let dom
let testModule
try {
  writeFileSync(join(out, 'fixture.mjs'), modules.entry)
  await build({
    configFile: false,
    logLevel: 'error',
    resolve: { alias: { '@': src } },
    plugins: [{
      name: 'drawing-session-fixture',
      enforce: 'pre',
      resolveId(id) {
        if (id.startsWith('test:')) return `\0${id}`
        if (id === '@/ipc/client' || id === `${src}/ipc/client`) return '\0test:ipc'
        if (id === '@/store/workspace' || id === `${src}/store/workspace`) return '\0test:workspace'
        if (id === './ExcalidrawSurface') return '\0test:surface'
      },
      load(id) { if (id.startsWith('\0test:')) return modules[id.slice('\0test:'.length)] },
    }],
    build: {
      ssr: join(out, 'fixture.mjs'), outDir: out, emptyOutDir: false,
      rollupOptions: { external: ['react', 'react-dom/client', 'zustand'], output: { entryFileNames: 'entry.mjs' } },
    },
  })
  dom = new JSDOM('<div id="root"></div>', { url: 'https://cide.test' })
  for (const key of ['window', 'document', 'HTMLElement', 'Element', 'Node']) {
    globalThis[key] = key === 'window' ? dom.window : dom.window[key]
  }
  globalThis.IS_REACT_ACT_ENVIRONMENT = true
  testModule = await import(pathToFileURL(join(out, 'entry.mjs')).href)
  const { ExcalidrawHost, drawingSessions, createDrawingHosts, useWorkspace, control, saveTab, registeredBuffers, readoutClaims } = testModule
  const timers = new Map()
  let timerId = -1
  const originalTimeout = window.setTimeout.bind(window)
  const originalClear = window.clearTimeout.bind(window)
  window.setTimeout = (fn, delay, ...args) => {
    if (delay >= 60_000) { const id = timerId--; timers.set(id, () => fn(...args)); return id }
    return originalTimeout(fn, delay, ...args)
  }
  window.clearTimeout = id => { if (!timers.delete(id)) originalClear(id) }

  const p = '/p/a.excalidraw', q = '/q/b.excalidraw'
  function record(path, version = 1) {
    const bytes = new TextEncoder().encode(JSON.stringify({
      elements: [{ id: path, version, isDeleted: false }], files: {},
      appState: { viewBackgroundColor: '#ffffff' },
    }))
    const old = control.records.get(path)
    control.records.set(path, { bytes, stamp: { mtimeNanos: String(Number(old?.stamp.mtimeNanos ?? 0) + 1), len: bytes.length } })
  }
  record(p); record(q)
  const tab = (id, path, pane) => ({ id, kind: { kind: 'file', path }, tree: { panes: { [pane]: { id: pane, kind: 'editor' } } } })
  const project = (id, path, pane) => ({ id, roots: [{ path: `/${id}` }], activeTab: `t-${id}`, tabs: [tab(`t-${id}`, path, pane)] })
  let boot = {
    role: { kind: 'shell', active: 'p', projects: ['p', 'q'] },
    workspace: {
      windows: {}, settings: { theme: 'dark', editor: { autosave: false } },
      projects: { p: project('p', p, 'pane-p'), q: project('q', q, 'pane-q') },
    },
  }
  function snapshot(change) {
    boot = structuredClone(boot); change(boot); useWorkspace.setState({ boot })
  }
  const waitFor = async predicate => {
    for (let i = 0; i < 100; i++) {
      if (predicate()) return
      await act(async () => { await new Promise(resolve => setTimeout(resolve, 10)) })
    }
    assert.fail('drawing did not settle')
  }
  let focus = 0, menus = 0
  outer = createRoot(document.getElementById('root'))
  async function show(id, onScreen = true) {
    await act(async () => {
      snapshot(b => { b.role.active = id; b.workspace.projects[id].activeTab = onScreen ? `t-${id}` : 'terminal' })
      const path = boot.workspace.projects[id].tabs[0].kind.path
      outer.render(React.createElement(StrictMode, null,
        React.createElement('div', { key: id, onPointerDownCapture: () => focus++, onContextMenu: () => menus++ },
          React.createElement(ExcalidrawHost, { pane: `pane-${id}`, project: id, tab: `t-${id}`, root: `/${id}`, path, onScreen }))))
    })
    await waitFor(() => document.querySelector('#root [data-canvas]'))
    return control.canvases.get(document.querySelector('#root [data-canvas]').dataset.canvas)
  }
  const emitTool = async path => act(async () => { for (const fn of control.tool) fn('agent', [path]) })
  const emitGate = async (editor, request, phase, restored = false) => act(async () => {
    const registration = control.historyMounts.get(editor);
    assert.ok(registration, 'the live editor registered its mount identity');
    for (const fn of control.history) fn({ editors: [registration], request, phase, restored });
  })
  const latestDirty = tab => control.dirty.filter(change => change.tab === tab).at(-1)?.dirty
  const diskVersion = path => JSON.parse(new TextDecoder().decode(control.records.get(path).bytes)).elements[0].version

  const a = await show('p')
  assert.equal(control.mounts, 1, 'StrictMode slot replay creates one engine; inactive projects are not opened')
  assert.deepEqual([...drawingSessions(boot).keys()], ['pane-p', 'pane-q'])
  const splitBoot = structuredClone(boot)
  splitBoot.workspace.projects.p.tabs[0].tree.panes.agent = { id: 'agent', kind: 'claude' }
  assert.deepEqual([...drawingSessions(splitBoot).keys()], ['pane-p', 'pane-q'], 'agent splits are not drawing sessions')
  const node = document.querySelector('[data-canvas]')
  const container = node.closest('[data-drawing-host]')
  container.getBoundingClientRect = () => ({ width: 800, height: 600 })
  const view = { scrollX: -120, scrollY: 340, zoom: { value: 2 }, selectedElementIds: { [p]: true }, activeTool: { type: 'arrow' } }
  await act(async () => { a.edit(); a.edit(); a.view(view) })
  assert.equal(latestDirty('t-p'), true)
  assert.equal(diskVersion(p), 1, 'autosave disabled leaves the unsaved scene in memory')
  await show('p', false)
  assert.equal(await show('p'), a, 'tab switching keeps the engine instance')
  const b = await show('q')
  assert.equal(node.closest('[data-drawing-parking]').getAttribute('aria-hidden'), 'true')
  assert.equal(container.style.width, '800px')
  assert.equal(container.style.height, '600px')
  assert.deepEqual(registeredBuffers().sort(), ['t-p', 't-q'], 'parked drawings still offer Save')
  assert.equal(await show('p'), a, 'project switching keeps the engine instance')
  assert.ok(control.refreshes.includes(a.id), 'reattaching refreshes canvas offsets without resetting the scene')
  assert.equal(document.querySelector('#root [data-canvas]'), node)
  for (const key of Object.keys(view)) assert.deepEqual(a.getAppState()[key], view[key], `preserved ${key}`)
  await act(async () => a.undo())
  assert.equal(a.getSceneElements()[0].version, 2, 'private undo survives project switching')
  await act(async () => a.redo())
  assert.equal(a.getSceneElements()[0].version, 3, 'private redo survives project switching')
  await act(async () => {
    node.dispatchEvent(new window.MouseEvent('pointerdown', { bubbles: true }))
    node.dispatchEvent(new window.MouseEvent('contextmenu', { bubbles: true, cancelable: true }))
  })
  assert.equal(focus, 1, 'reattached nested roots still raise pane focus')
  assert.equal(menus, 0, 'drawing context menu does not open the pane menu')
  assert.equal(control.mounts, 2)

  await emitGate('pane-p', 'dirty-restore', 'prepare')
  assert.equal(a.getAppState().viewModeEnabled, true, 'drawing freezes before a live dirty acknowledgement')
  assert.equal(control.acks.at(-1).dirty, true)
  const writesBeforeGate = control.writes.length
  await act(async () => { await saveTab('t-p') })
  assert.equal(control.writes.length, writesBeforeGate, 'a gated drawing cannot save')
  await emitGate('pane-p', 'dirty-restore', 'release')
  assert.equal(a.getAppState().viewModeEnabled, false)
  assert.equal(control.canvases.get(a.id), a, 'refused restore retains the engine and undo stack')

  await show('q')
  await act(async () => { await saveTab('t-p') })
  assert.equal(diskVersion(p), 3, 'Save all/close can save a parked drawing')
  assert.equal(latestDirty('t-p'), false)
  await act(async () => {
    snapshot(boot => { boot.workspace.settings.editor.autosave = true })
  })
  await act(async () => a.edit())
  await act(async () => { for (const [id, run] of timers) { timers.delete(id); run() } })
  assert.equal(diskVersion(p), 4, 'idle autosave remains active in a parked project')
  assert.equal(latestDirty('t-p'), false)
  await show('p')
  const reads = control.reads.length
  await emitTool(p)
  assert.equal(control.reads.length, reads, 'unchanged tool notifications never reload the drawing')
  assert.equal(control.canvases.get(a.id), a)
  await show('q')
  await emitTool(p)
  await act(async () => { for (const fn of control.git) fn() })
  assert.equal(await show('p'), a, 'hidden tool/git notifications check unchanged files without resetting the session')
  assert.equal(control.reads.length, reads)
  await act(async () => a.edit())
  await emitTool(p)
  assert.equal(document.querySelector('[role="alert"]'), null, 'an unchanged file does not raise a dirty conflict')
  record(p, 20)
  await emitTool(p)
  assert.ok(document.querySelector('[role="alert"]'), 'a changed file raises a conflict without losing unsaved edits')
  assert.equal(control.canvases.get(a.id), a)
  const button = text => [...document.querySelectorAll('button')].find(button => button.textContent === text)
  await act(async () => button('Keep mine').click())
  await act(async () => { await saveTab('t-p') })
  assert.equal(diskVersion(p), 5)

  // A slow stat of an earlier disk version must not raise a conflict after our save.
  let finishStat
  control.nextStat = new Promise(resolve => { finishStat = resolve })
  await emitTool(p)
  await act(async () => { a.edit(); await saveTab('t-p') })
  await act(async () => finishStat({ mtimeNanos: '999', len: 1 }))
  assert.equal(document.querySelector('[role="alert"]'), null)
  assert.equal(control.canvases.get(a.id), a)

  record(p, 30)
  await emitTool(p)
  await waitFor(() => !control.canvases.has(a.id))
  const reloaded = control.canvases.get(document.querySelector('#root [data-canvas]').dataset.canvas)
  assert.equal(reloaded.getSceneElements()[0].version, 30, 'clean external changes reload the drawing')
  assert.equal(reloaded.getAppState().zoom.value, 2, 'a genuine reload carries the viewport')
  await act(async () => reloaded.undo())
  assert.equal(reloaded.getSceneElements()[0].version, 30, 'reload cannot undo into the discarded scene')
  await act(async () => button('Reload').click())
  await waitFor(() => !control.canvases.has(reloaded.id))
  let current = control.canvases.get(document.querySelector('#root [data-canvas]').dataset.canvas)
  await emitGate('pane-p', 'clean-restore', 'prepare')
  assert.equal(control.acks.at(-1).dirty, false)
  assert.equal(current.getAppState().viewModeEnabled, true)
  const beforeRestore = current
  record(p, 40)
  await emitGate('pane-p', 'clean-restore', 'release', true)
  await waitFor(() => !control.canvases.has(beforeRestore.id))
  current = control.canvases.get(document.querySelector('#root [data-canvas]').dataset.canvas)
  assert.equal(current.getSceneElements()[0].version, 40, 'successful restore reloads disk bytes')
  assert.equal(current.getAppState().viewModeEnabled, false, 'replacement engine releases the restore gate')
  await act(async () => current.undo())
  assert.equal(current.getSceneElements()[0].version, 40, 'restored drawing cannot undo into pre-restore contents')

  await show('q')
  const renamed = '/p/renamed.excalidraw.json'
  control.records.set(renamed, control.records.get(p)); control.records.delete(p)
  await act(async () => snapshot(boot => { boot.workspace.projects.p.tabs[0].kind.path = renamed }))
  await act(async () => { current.edit(); await saveTab('t-p') })
  assert.equal(control.writes.at(-1), renamed, 'a parked session follows a rename before saving')
  assert.equal(await show('p'), current)
  await show('q')
  await act(async () => current.edit())
  assert.ok(timers.size > 0)
  await act(async () => snapshot(boot => { boot.workspace.projects.p.tabs = [] }))
  assert.equal(control.canvases.has(current.id), false, 'closing a parked tab disposes its engine')
  assert.equal(saveTab('t-p'), null)
  assert.equal(readoutClaims().length, 1, 'close releases the status claim')
  assert.equal(timers.size, 0, 'close cancels the retained autosave timer')

  // Ownership uses the window's complete assignment and the existing detached-tab filter.
  const detached = { kind: 'detachedTab', project: 'q', tab: 't-q' }
  const detachedBoot = structuredClone(boot)
  detachedBoot.workspace.windows.detached = detached
  assert.equal(drawingSessions(detachedBoot).size, 0, 'shell does not retain a detached tab')
  detachedBoot.role = detached
  assert.deepEqual([...drawingSessions(detachedBoot).keys()], ['pane-q'])
  detachedBoot.role = { kind: 'detachedPane', project: 'q', tab: 't-q' }
  assert.equal(drawingSessions(detachedBoot).size, 0)
  const noDrawing = structuredClone(boot)
  noDrawing.workspace.projects.q.tabs[0].kind.path = '/q/drawing.txt'
  assert.equal(drawingSessions(noDrawing).size, 0, 'renaming to another file kind releases its drawing session')
  noDrawing.workspace.projects.q.tabs[0].kind.path = q
  noDrawing.role.projects = ['p']
  assert.equal(drawingSessions(noDrawing).size, 0, 'a project assigned to another window is not retained here')
  await act(async () => {
    outer.render(null)
    snapshot(boot => { delete boot.workspace.projects.q })
  })
  assert.equal(control.canvases.has(b.id), false, 'closing a project releases parked drawings')
  assert.deepEqual(registeredBuffers(), [])
  assert.equal(document.querySelector('[data-drawing-parking]'), null)
  assert.equal(control.mounts, control.unmounts)
  assert.equal(control.tool.size, 0)
  assert.equal(control.git.size, 0)
  assert.equal(control.history.size, 0)
  assert.equal(control.historyMounts.size, 0)

  // A late cleanup from an old split slot must not steal a newer attachment.
  const registry = createDrawingHosts(props => React.createElement('span', null, props.path))
  const first = document.createElement('div'), second = document.createElement('div')
  document.body.append(first, second)
  let releaseOld, releaseNew
  await act(async () => {
    releaseOld = registry.mount('moving', first, { path: 'first', onScreen: true })
    releaseNew = registry.mount('moving', second, { path: 'second', onScreen: true })
    releaseOld()
  })
  assert.equal(second.textContent, 'second')
  await act(async () => { releaseNew(); registry.reconcile(new Map()) })
  first.remove(); second.remove()
  console.log('Excalidraw sessions: tab/project switches, undo, saving, reloads, restore gates, rename, ownership and cleanup: ok')
} finally {
  if (outer) await act(async () => outer.unmount())
  if (testModule) await act(async () => {
    const { useWorkspace } = testModule
    const boot = useWorkspace.getState().boot
    if (boot === null) return
    // Parked roots can read settings before the queued workspace reconciliation disposes them.
    // Close their projects while retaining the rest of the last valid bootstrap snapshot.
    useWorkspace.setState({ boot: {
      ...boot,
      role: { kind: 'shell', active: null, projects: [] },
      workspace: { ...boot.workspace, projects: {} },
    } })
  })
  dom?.window.close()
  rmSync(out, { recursive: true, force: true })
}
