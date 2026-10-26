/** Real CodeMirror documents, merge controls and shared-view synchronization; IPC is in memory. */
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, writeFileSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'
import { JSDOM } from 'jsdom'

const ui = resolve(import.meta.dirname, '..')
mkdirSync(join(ui, 'node_modules/.cache'), { recursive: true })
const out = mkdtempSync(join(ui, 'node_modules/.cache/clipboard-diff-'))
const dom = new JSDOM('<!doctype html><body></body>', { url: 'https://cide.test', pretendToBeVisual: true })
for (const name of ['Window', 'window', 'document', 'location', 'HTMLElement', 'Element', 'Node', 'MutationObserver', 'MouseEvent']) globalThis[name] = dom.window[name]
globalThis.getComputedStyle = dom.window.getComputedStyle.bind(dom.window)
globalThis.requestAnimationFrame = dom.window.requestAnimationFrame.bind(dom.window)
globalThis.cancelAnimationFrame = dom.window.cancelAnimationFrame.bind(dom.window)
globalThis.ResizeObserver = class { observe() {} disconnect() {} unobserve() {} }
dom.window.Range.prototype.getClientRects = () => []
dom.window.Range.prototype.getBoundingClientRect = () => ({ top: 0, left: 0, bottom: 20, right: 400, width: 400, height: 20 })
const snapshots = new Map(), writes = [], errors = []
let changed, workspaceChanged
globalThis.__clipboardDiffTest = {
  onChanged: fn => { changed = fn; return Promise.resolve(() => {}) },
  onWorkspaceChanged: fn => { workspaceChanged = fn; return Promise.resolve(() => {}) },
  sync: async (path, expected, next) => {
    const current = snapshots.get(path)
    if (next && (current?.version ?? null) === expected) {
      const result = { ...next, version: (expected ?? 0) + 1 }
      snapshots.set(path, result)
      queueMicrotask(() => changed?.({ path, version: result.version }))
    }
    return snapshots.get(path) ?? null
  },
  write: async (path, text, stamp) => { writes.push({ path, text, stamp }); return { mtimeNanos: '2', len: text.length } },
  notify: text => errors.push(text),
}
const views = []
const tick = () => new Promise(r => setTimeout(r, 35))
try {
  const entry = join(out, 'entry.ts')
  writeFileSync(entry, `
    export * from '@/editor/sharedDocumentModel';
    export * from '@/editor/sharedDocuments';
    export * from '@/editor/clipboardDiffView';
    export { EditorState } from '@codemirror/state';
    export { EditorView } from '@codemirror/view';
    export { isolateHistory } from '@codemirror/commands';
  `)
  await build({ configFile: false, root: ui, logLevel: 'error',
    resolve: { alias: [
      { find: '@/store/workspace', replacement: '\0clipboard-store' },
      { find: '@/ipc/client', replacement: '\0clipboard-ipc' },
      { find: '@/chrome/notices', replacement: '\0clipboard-notices' },
      { find: /^\.\/languages$/, replacement: '\0clipboard-languages' },
      { find: '@', replacement: join(ui, 'src') },
    ] },
    plugins: [{ name: 'clipboard-fixture',
      resolveId(id) { if (id.startsWith('\0clipboard-')) return id },
      load(id) {
        if (id === '\0clipboard-ipc') return `const h = globalThis.__clipboardDiffTest;
          export const sharedDocument = { sync: h.sync, onChanged: h.onChanged };
          export const events = { onWorkspaceChanged: h.onWorkspaceChanged };
          export const file = { write: h.write };`
        if (id === '\0clipboard-store') return 'export const useWorkspace = { subscribe: () => () => {} };'
        if (id === '\0clipboard-notices') return 'export const notify = globalThis.__clipboardDiffTest.notify;'
        if (id === '\0clipboard-languages') return 'export const loadLanguage = async () => null;'
      },
    }], build: { ssr: entry, outDir: join(out, 'bundle') },
  })
  const m = await import(pathToFileURL(join(out, 'bundle/entry.js')).href)
  const text = model => model.state.doc.toString()
  const edited = (model, changes) => model.edit(model.state.update({ changes, annotations: m.isolateHistory.of('full') }))
  const model = new m.SharedDocumentModel('one\ntwo\nthree\n')
  edited(model, { from: 4, to: 7, insert: 'clipboard' })
  assert.equal(text(model), 'one\nclipboard\nthree\n')
  assert.equal(model.dirty, true)
  assert.equal(model.history('undo'), true)
  assert.equal(text(model), 'one\ntwo\nthree\n')
  assert.equal(model.dirty, false)
  model.history('redo')
  model.saved(text(model), { mtimeNanos: '1', len: text(model).length })
  assert.equal(model.dirty, false)
  model.history('undo')
  assert.equal(model.dirty, true, 'Undo after save dirties the shared document')

  // Two independent windows edit one revision. The loser rebases rather than dropping work.
  const left = new m.SharedDocumentModel('ab'), right = new m.SharedDocumentModel('ab')
  const initial = { ...left.snapshot(), version: 1 }
  left.receive(initial); right.receive(initial)
  edited(left, { from: 0, insert: 'L' }); edited(right, { from: 2, insert: 'R' })
  const first = { ...left.snapshot(), version: 2 }
  left.receive(first, left.submission().count)
  right.receive(first)
  assert.equal(text(right), 'LabR', 'Concurrent non-overlapping edits both survive')
  const second = { ...right.snapshot(), version: 3 }
  right.receive(second, right.submission().count); left.receive(second)
  assert.equal(text(left), text(right))
  left.history('undo')
  assert.equal(text(left), 'Lab', 'The shared history includes the other window’s edit')

  // A save acknowledgement must not absorb edits typed while it was in flight.
  const inFlight = new m.SharedDocumentModel('a')
  inFlight.receive({ ...inFlight.snapshot(), version: 1 })
  edited(inFlight, { from: 1, insert: 'b' })
  const sent = inFlight.submission()
  edited(inFlight, { from: 2, insert: 'c' })
  inFlight.receive({ ...sent.next, version: 2 }, sent.count)
  assert.equal(text(inFlight), 'abc')
  inFlight.saved('ab', null)
  assert.equal(inFlight.dirty, true)
  const recovery = new m.SharedDocumentModel('on disk', { mtimeNanos: 'old', len: 7 })
  edited(recovery, { from: 0, to: 7, insert: 'recovered' })
  assert.equal(recovery.snapshot().stamp.mtimeNanos, 'old')
  assert.equal(recovery.dirty, true)

  const typing = new m.SharedDocumentModel('')
  typing.receive({ ...typing.snapshot(), version: 1 })
  for (const char of 'word') {
    typing.edit(typing.state.update({ changes: { from: text(typing).length, insert: char }, userEvent: 'input.type' }))
    const sent = typing.submission()
    typing.receive({ ...sent.next, version: (sent.expected ?? 0) + 1 }, sent.count)
  }
  assert.equal(typing.undoDepth, 1, 'Native acknowledgements preserve the typing group')
  typing.history('undo')
  assert.equal(text(typing), '')

  // Two mounted editors use the same live buffer and undo history.
  const path = '/clipboard-test.txt'
  await m.prepareDocument(path, 'file\n', { mtimeNanos: '1', len: 5 }, 'file-tab')
  const parent = () => { const el = document.createElement('div'); document.body.append(el); return el }
  const a = new m.EditorView({ parent: parent(), doc: 'file\n' })
  const b = new m.EditorView({ parent: parent(), doc: 'file\n' })
  views.push(a, b)
  let dirtyA = false, dirtyB = false
  const stopA = m.attachDocument(path, a, saved => { dirtyA = a.state.doc.toString() !== saved })
  const stopB = m.attachDocument(path, b, saved => { dirtyB = b.state.doc.toString() !== saved })
  a.dispatch({ changes: { from: 0, to: 4, insert: 'unsaved' } })
  assert.equal(b.state.doc.toString(), 'unsaved\n')
  assert.equal(dirtyA && dirtyB, true)
  m.sharedHistory(b, 'undo')
  assert.equal(a.state.doc.toString(), 'file\n')
  m.sharedHistory(a, 'redo')
  assert.equal(b.state.doc.toString(), 'unsaved\n')
  await m.saveSharedDocument(path, b.state.doc.toString(), null)
  assert.equal(dirtyA || dirtyB, false)
  assert.equal(writes.at(-1).text, 'unsaved\n')
  const writeCount = writes.length
  await Promise.all([m.saveSharedDocument(path, 'unsaved\n', null), m.saveSharedDocument(path, 'unsaved\n', null)])
  assert.equal(writes.length, writeCount, 'Two views do not autosave the same bytes twice')
  stopA(); a.destroy(); views.splice(views.indexOf(a), 1)
  b.dispatch({ changes: { from: 0, insert: 'still here: ' } })
  assert.equal(m.documentFor(path).state.doc.toString(), 'still here: unsaved\n')
  await m.flushDocument(path)

  // Rename between typing and the next native flush, then remount the comparison view.
  b.dispatch({ changes: { from: 0, insert: 'rename: ' } })
  const renamed = '/renamed.txt'
  snapshots.set(renamed, snapshots.get(path)); snapshots.delete(path)
  workspaceChanged({ projects: { p: { tabs: [{ id: 'file-tab', kind: { kind: 'file', path: renamed } }] } } })
  await m.flushDocuments()
  assert.equal(m.documentFor(path), undefined)
  assert.equal(JSON.parse(snapshots.get(renamed).state).doc, b.state.doc.toString())
  stopB()
  const doc = await m.prepareDocument(renamed, 'file\n', null, 'comparison-tab')
  const diffHost = parent()
  const remounted = m.createClipboardDiff(diffHost, renamed, 'file\n', [], {
    id: 'later', text: 'clipboard\n', layout: 'split',
  }, false)
  const stopDiff = m.attachDocument(renamed, remounted.view, () => {})
  assert.equal(remounted.view.state.doc.toString(), text(doc), 'Remount uses the unsaved shared document')
  await tick()
  diffHost.querySelector('button[aria-label="Copy from Clipboard"]').dispatchEvent(new MouseEvent('mousedown', { bubbles: true }))
  assert.equal(text(doc), 'clipboard\n')
  assert.equal(m.sharedHistory(remounted.view, 'undo'), true)
  assert.equal(text(doc), b.state.doc.toString(), 'A copied hunk is one shared Undo step')
  stopDiff(); remounted.destroy(); diffHost.remove()

  // A late native update must be read before saving, even if its event has not arrived yet.
  const stalePath = '/stale-save.txt'
  await m.prepareDocument(stalePath, 'before', null)
  const remote = new m.SharedDocumentModel('before')
  remote.receive(snapshots.get(stalePath))
  edited(remote, { from: 6, insert: ' from another window' })
  snapshots.set(stalePath, { ...remote.snapshot(), version: 2 })
  const beforeWrites = writes.length
  await assert.rejects(m.saveSharedDocument(stalePath, 'before', null), /changed while preparing to save/)
  assert.equal(writes.length, beforeWrites, 'Saving a stale view cannot overwrite a remote edit')
  assert.equal(text(m.documentFor(stalePath)), 'before from another window')

  // Real merge controls apply a block to the file, never alter the clipboard snapshot.
  for (const layout of ['split', 'unified']) {
    for (const [original, fileText] of [['new\n', 'old\n'], ['new\n', ''], ['', 'old\n'], ['🌍\n', '🌎\n']]) {
      const host = parent()
      const diff = m.createClipboardDiff(host, '/test.txt', fileText, [], { id: 'snapshot', text: original, layout }, false)
      await tick()
      const control = host.querySelector('button[aria-label="Copy from Clipboard"]')
      assert.ok(control, `${layout}: block copy control exists`)
      control.dispatchEvent(new MouseEvent(layout === 'split' ? 'mousedown' : 'click', { bubbles: true }))
      assert.equal(diff.view.state.doc.toString(), original, `${layout}: insertion/replacement/deletion`)
      if (diff.clipboard) assert.equal(diff.clipboard.state.doc.toString(), original)
      diff.destroy(); host.remove()
    }
    const host = parent()
    const readonly = m.createClipboardDiff(host, '/test.txt', 'old', [], { id: 'r', text: 'new', layout }, true)
    await tick()
    assert.equal(host.querySelector('button[aria-label="Copy from Clipboard"]'), null)
    assert.equal(readonly.view.state.readOnly, true)
    readonly.destroy(); host.remove()
    for (const value of ['', 'unchanged\r\n']) {
      const host = parent()
      const equal = m.createClipboardDiff(host, '/equal.txt', value, [], { id: 'equal', text: value, layout }, false)
      await tick()
      assert.equal(host.querySelector('button[aria-label="Copy from Clipboard"]'), null, 'Identical and empty documents have no copy controls')
      equal.destroy(); host.remove()
    }
  }
  assert.equal(m.tooLargeForClipboardDiff('x'.repeat(1024 * 1024), 'x'), true)
  assert.equal(m.tooLargeForClipboardDiff('\n'.repeat(12_000), ''), true)
  assert.equal(m.tooLargeForClipboardDiff('', ''), false)
  assert.deepEqual(errors, [])
  workspaceChanged?.({ projects: {} })
  console.log('clipboard comparison: shared editing, history, concurrency, saves and merge controls passed')
} finally {
  for (const view of views) view.destroy()
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
