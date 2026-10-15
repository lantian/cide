/** Exercise the real zustand store with a fake IPC boundary; no browser clipboard or DOM. */
import assert from 'node:assert/strict'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'

const ui = resolve(import.meta.dirname, '..')
const out = mkdtempSync(join(ui, 'node_modules', '.cache', 'file-clipboard-store-'))
const tick = () => new Promise((resolve) => setImmediate(resolve))
const deferred = () => {
  let resolve, reject
  const promise = new Promise((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}
const makeClip = (id, mode = 'copy') => ({ id, mode, project: 'source', paths: [`/source/${id}`] })
let current = null
let changed
let focus
let readGate = null
let writeGate = null
let pasteGate = null
let reads = 0
let writes = 0
const plans = []
const pastes = []
const oldWindow = globalThis.window
const oldTest = globalThis.__fileClipboardTest

globalThis.window = { addEventListener: (name, callback) => { if (name === 'focus') focus = callback } }
globalThis.__fileClipboardTest = {
  fileClip: {
    onChanged: (callback) => { changed = callback; return Promise.resolve(() => {}) },
    get: async () => { reads++; return readGate === null ? current : await readGate.promise },
    set: async (draft) => {
      writes++
      if (writeGate !== null) await writeGate.promise
      current = { ...draft, id: `write-${writes}` }
      return current
    },
    clear: async (id) => { if (current?.id === id) current = null },
  },
  fsClipboard: {
    plan: async (...args) => { plans.push(args); return [] },
    paste: async (...args) => {
      pastes.push(args)
      if (args[6] !== null && args[6] !== current?.id) throw new Error('clipboard changed')
      if (pasteGate !== null) return await pasteGate.promise
      return [{ source: args[1][0], dest: '/dest/pasted', renamed: false, skipped: 0, replaced: 0 }]
    },
  },
  diag: { log: async () => {} },
}

try {
  const bundle = await build({
    configFile: false, logLevel: 'silent',
    plugins: [{
      name: 'file-clipboard-test-ipc',
      resolveId: (id) => id === '@/ipc/client' ? '\0file-clipboard-test-ipc' : null,
      load: (id) => id === '\0file-clipboard-test-ipc'
        ? 'export const { fileClip, fsClipboard, diag } = globalThis.__fileClipboardTest' : null,
    }],
    build: {
      write: false, minify: false,
      lib: { entry: join(ui, 'src/sidebar/fileClipboard.ts'), formats: ['es'] },
      rollupOptions: { external: ['zustand'] },
    },
  })
  const result = Array.isArray(bundle) ? bundle[0] : bundle
  const entry = result.output.find((file) => file.type === 'chunk' && file.isEntry)
  const path = join(out, 'store.mjs')
  writeFileSync(path, entry.code)
  const { useFileClipboard: store, followFileClip, planEntries, pasteEntries } = await import(pathToFileURL(path))
  followFileClip()
  await tick()
  assert.equal(store.getState().clip, null)

  // Copy followed immediately by paste refresh waits for the native write.
  const beforeReads = reads
  writeGate = deferred()
  const taking = store.getState().take('copy', 'source', ['/source/first'])
  const refreshing = store.getState().refresh()
  await tick()
  assert.equal(reads, beforeReads)
  writeGate.resolve()
  await taking
  await refreshing
  writeGate = null
  assert.deepEqual(store.getState().clip, current)

  current = makeClip('from-another-instance')
  focus()
  await tick()
  assert.deepEqual(store.getState().clip, current, 'desktop focus imports a foreign clipboard')

  // A slow read must not overwrite a newer event from another window.
  readGate = deferred()
  const staleRead = store.getState().refresh()
  await tick()
  current = makeClip('newer-event')
  changed(current)
  readGate.resolve(makeClip('stale'))
  assert.deepEqual(await staleRead, current)
  readGate = null

  const snapshot = current
  current = makeClip('replaced-while-dialog-open')
  changed(current)
  await assert.rejects(store.getState().paste('dest', '/dest', [], snapshot), /clipboard changed/)
  assert.equal(pastes.at(-1)[6], snapshot.id, 'execution refers to the displayed snapshot')
  assert.deepEqual(store.getState().clip, current)

  // A newer Copy made during a long Cut paste must survive the successful move.
  current = makeClip('moving', 'cut')
  changed(current)
  pasteGate = deferred()
  const moving = store.getState().paste('dest', '/dest', [], current)
  current = makeClip('copy-during-move')
  changed(current)
  pasteGate.resolve([{ source: '/source/moving', dest: '/dest/moving', renamed: false, skipped: 0, replaced: 0 }])
  await moving
  pasteGate = null
  assert.deepEqual(store.getState().clip, current)

  // Failed moves leave the pending Cut available for a retry.
  current = makeClip('retry-move', 'cut')
  changed(current)
  pasteGate = deferred()
  const failedMove = store.getState().paste('dest', '/dest', [], current)
  pasteGate.reject(new Error('move failed'))
  await assert.rejects(failedMove, /move failed/)
  pasteGate = null
  assert.deepEqual(store.getState().clip, current)

  // Escape may have been pressed against an old mirror before another instance's Copy arrived.
  const pendingCut = current
  current = makeClip('copy-before-cancel')
  await store.getState().clear()
  assert.notEqual(current.id, pendingCut.id)
  assert.deepEqual(store.getState().clip, current)

  current = makeClip('consumed', 'cut')
  changed(current)
  await store.getState().paste('dest', '/dest', [], current)
  assert.equal(store.getState().clip, null)
  current = makeClip('repeatable')
  changed(current)
  await store.getState().paste('dest', '/dest', [], current)
  await store.getState().paste('dest', '/dest', [], current)
  assert.deepEqual(store.getState().clip, current)

  const prior = current
  writeGate = deferred()
  const failing = store.getState().take('copy', 'source', ['/source/failure'])
  await tick()
  writeGate.reject(new Error('native write failed'))
  await assert.rejects(failing, /native write failed/)
  writeGate = null
  assert.deepEqual(store.getState().clip, prior)
  await store.getState().refresh() // a failed native write cannot poison the operation queue

  const priorWrites = writes
  await planEntries('dest', ['/dest/drag'], '/dest/folder', 'cut')
  await pasteEntries('dest', ['/dest/drag'], '/dest/folder', 'cut', [])
  assert.equal(plans.at(-1)[5], null)
  assert.equal(pastes.at(-1)[6], null)
  assert.equal(writes, priorWrites, 'drag transfers never replace the native clipboard')
  assert.deepEqual(store.getState().clip, prior)
  console.log('file clipboard store ordering and snapshots: ok')
} finally {
  globalThis.window = oldWindow
  globalThis.__fileClipboardTest = oldTest
  rmSync(out, { recursive: true, force: true })
}
