/** Regression checks for restored Codex launches and the recovery splash. */
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, writeFileSync, rmSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'

const UI = resolve(import.meta.dirname, '..')
mkdirSync(join(UI, 'node_modules/.cache'), { recursive: true })
const out = mkdtempSync(join(UI, 'node_modules/.cache', 'cide-restore-'))
try {
  const entry = join(out, 'restore.tsx')
  writeFileSync(entry, `
    import { createElement } from 'react'
    import { renderToStaticMarkup } from 'react-dom/server'
    import { ResumeSplash } from '${join(UI, 'src/windows/ResumeSplash.tsx')}'
    export { ResumeSplash }
    export { specFor, restartSpec } from '${join(UI, 'src/panes/terminalSpec.ts')}'
    export const splash = (props) => renderToStaticMarkup(createElement(ResumeSplash, props))
  `)
  await build({
    root: UI,
    logLevel: 'error',
    build: { ssr: entry, outDir: join(out, 'bundle'), emptyOutDir: true },
  })
  const { specFor, restartSpec, splash, ResumeSplash } = await import(pathToFileURL(join(out, 'bundle/restore.js')))
  const pane = {
    id: 'pane', kind: 'claude', role: 'primary', title: 'test9 : codex',
    session: 'routing-id', harness: 'codex', conversation: null, continues: null,
  }
  const plan = (restore) => ({ pane: pane.id, kind: 'claude', cwd: '/worktree', restore })
  const resumable = plan({ kind: 'resumable', session: 'saved-thread' })
  assert.equal(specFor(pane, '/project', 'project', resumable).resume, 'saved-thread')
  const continued = { ...pane, harness: null, continues: { harness: 'codex', id: 'older-thread', cwd: '/worktree' } }
  const launch = specFor(continued, '/worktree', 'project', resumable)
  assert.equal(launch.continues.id, 'saved-thread', 'restore must use the latest thread')
  assert.equal(launch.continues.cwd, '/worktree')
  const missing = plan({ kind: 'missingConversation' })
  const picker = specFor(continued, '/worktree', 'project', missing, 'picker')
  assert.equal(picker.program, 'codex')
  assert.equal(picker.resumePicker, true)
  assert.equal(picker.resume, undefined)
  assert.equal(picker.continues, undefined, 'an old continuation must not override the picker')
  const fresh = specFor(continued, '/worktree', 'project', missing, 'fresh')
  assert.equal(fresh.program, 'codex')
  assert.equal(fresh.resumePicker, false)
  assert.equal(fresh.resume, undefined)
  assert.equal(fresh.continues, undefined)
  const openCode = { ...pane, harness: 'opencode', harnessConversation: { harness: 'opencode', id: 'ses_native', cwd: '/worktree' } }
  assert.equal(specFor(openCode, '/worktree', 'project', plan({ kind: 'resumable', session: 'routing-id' })).resume, 'routing-id')
  assert.equal(specFor(openCode, '/worktree', 'project', missing).resumePicker, undefined)
  const restarted = restartSpec({ ...launch, program: 'opencode', args: ['--session', 'ses_native'] }, 'claude', 'fresh', 'routing-id')
  assert.equal(restarted.program, 'claude', 'a fresh console restart resolves the current project default')
  assert.deepEqual(restarted.args, [])
  assert.equal(restarted.continues, undefined)
  assert.equal(restarted.resume, undefined)
  const resumedNative = restartSpec({ ...launch, program: 'claude' }, 'claude', 'resume', 'routing-id')
  assert.equal(resumedNative.resume, 'routing-id', 'the backend resolves the native conversation from the pane')
  assert.equal(resumedNative.continues, undefined)
  assert.equal(specFor(pane, '/project').resume, undefined)
  assert.equal(specFor({ ...pane, kind: 'editor' }, '/project'), null)
  const shell = specFor({ ...pane, kind: 'shell' }, '/project', 'project', plan({ kind: 'fresh' }))
  assert.equal(shell.program, '')
  assert.equal(shell.resume, 'routing-id')
  const normal = { title: 'test9 : codex', harness: 'codex', lastActive: null, cwd: '/project', onResume() {} }
  const html = splash(normal)
  assert.match(html, /Codex/)
  assert.doesNotMatch(html, /Claude Code/)
  assert.match(html, /Resume &quot;test9 : codex&quot;/)
  const recovery = splash({ ...normal, missingConversation: true, onFresh() {} })
  assert.match(recovery, /Choose a Codex session/)
  assert.match(recovery, /Start a new session/)
  assert.doesNotMatch(recovery, /Resume &quot;/)
  assert.match(splash({ ...normal, harness: undefined }), /Claude Code/)
  const { JSDOM } = await import('jsdom')
  const dom = new JSDOM('<div id="root"></div>', { url: 'http://localhost' })
  globalThis.window = dom.window
  globalThis.document = dom.window.document
  const { createElement } = await import('react')
  const { flushSync } = await import('react-dom')
  const { createRoot } = await import('react-dom/client')
  const root = createRoot(document.getElementById('root'))
  const selected = []
  flushSync(() => root.render(createElement(ResumeSplash, {
    ...normal, missingConversation: true,
    onResume: () => selected.push('picker'), onFresh: () => selected.push('fresh'),
  })))
  assert.deepEqual(selected, [], 'recovery must wait for a user choice')
  const buttons = [...document.querySelectorAll('button')]
  flushSync(() => buttons.find((b) => b.textContent === 'Choose a Codex session').click())
  assert.deepEqual(selected, ['picker'])
  flushSync(() => buttons.find((b) => b.textContent === 'Start a new session').click())
  assert.deepEqual(selected, ['picker', 'fresh'])
  flushSync(() => root.unmount())
  dom.window.close()
  delete globalThis.window
  delete globalThis.document
  console.log('restore: saved threads, latest continuation, picker, fresh recovery, shell, and splash labels passed')
} finally {
  rmSync(out, { recursive: true, force: true })
}
