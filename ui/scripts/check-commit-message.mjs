/** Run the real draft store and mounted commit panel against deferred, mocked IPC. */
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { resolve } from 'node:path'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync('node_modules/.cache/cide-commit-message-')
const dom = new JSDOM('<!doctype html><div id="root"></div>', {
  url: 'https://cide.test', pretendToBeVisual: true,
})
for (const key of ['window', 'document', 'location', 'HTMLElement', 'Element', 'Node', 'MutationObserver'])
  globalThis[key] = dom.window[key]
globalThis.IS_REACT_ACT_ENVIRONMENT = true
const { createRoot } = await import('react-dom/client')
const deferred = () => {
  let resolve, reject
  const promise = new Promise((yes, no) => { resolve = yes; reject = no })
  return { promise, resolve, reject }
}
const answer = (text) => ({ text, isError: false, subtype: 'success' })
const change = (path, staged = false) => ({
  path, origPath: null, index: staged ? 'modified' : 'unmodified', worktree: 'modified',
  staged, binary: false, submodule: false, changelist: 'default',
})
const repo = (id, staged = false) => ({
  repo: { id, name: id, root: `/test/${id}`, parent: null, isSubmodule: false },
  branch: { head: 'main', detached: false, upstream: null, ahead: 0, behind: 0,
    operation: null, unborn: false },
  changelists: [{ id: 'default', name: 'Changes', active: true, comment: '',
    changes: [change('picked.txt', staged), change('unchecked.txt', staged)] }],
  unversioned: [], ignored: [], conflicts: [], indexChangedExternally: false,
  useStagingArea: staged,
})
let status = { repos: [repo('repo-a')] }
const calls = []
let generation = deferred()
let commit = deferred()
window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} }
window.__TAURI_INTERNALS__ = {
  transformCallback: () => 1,
  unregisterCallback: () => {},
  invoke: async (command, args) => {
    calls.push({ command, args })
    if (command === 'git_status') return structuredClone(status)
    if (command === 'git_conflicts') return null
    if (command === 'git_shelf_list') return []
    if (command === 'claude_commit_message') return generation.promise
    if (command === 'git_commit') return commit.promise
    if (command.startsWith('plugin:event|')) return 1
    if (command === 'diag_log') return
    throw new Error(`Unexpected IPC: ${command}`)
  },
}

let root
try {
  execFileSync('node', ['node_modules/vite/bin/vite.js', 'build', '--ssr',
    'src/sidebar/GitPanel/commitSmoke.ts', '--outDir', out, '--logLevel', 'error'], { stdio: 'inherit' })
  const { CommitDraftStore, commitDrafts, commitDraftKey, useGitPanel, GitPanelView,
    setPartial, clearAllPartials } = await import(`file://${resolve(out, 'commitSmoke.js')}`)

  // Scope isolation, subscription cleanup, and edits that return to the same text.
  const drafts = new CommitDraftStore()
  const a = commitDraftKey('project', null)
  const b = commitDraftKey('project', '/worktree')
  let notifications = 0
  const unsubscribe = drafts.subscribe(a, () => notifications++)
  drafts.setMessage(a, 'typed draft')
  drafts.setMessage(b, 'worktree draft')
  assert.equal(notifications, 1)
  assert.equal(drafts.get(a).message, 'typed draft')
  unsubscribe()
  drafts.setMessage(a, 'another draft')
  assert.equal(notifications, 1)
  assert.equal(drafts.get(b).message, 'worktree draft')
  assert.equal(new CommitDraftStore().get(a).message, '', 'drafts do not survive a new window')

  let pending = deferred()
  const running = drafts.generate(a, () => pending.promise)
  let duplicates = 0
  await drafts.generate(a, () => { duplicates++; return Promise.resolve(answer('duplicate')) })
  assert.equal(duplicates, 0)
  drafts.setMessage(a, 'newer edit')
  pending.resolve(answer(' generated replacement \n'))
  await running
  assert.equal(drafts.get(a).message, 'newer edit')
  assert.equal(drafts.get(a).generated, 'generated replacement')
  const oldRevision = drafts.get(a).revision
  drafts.setMessage(a, 'latest edit')
  drafts.replaceGenerated(a, oldRevision)
  assert.equal(drafts.get(a).message, 'latest edit', 'stale confirmation cannot replace new work')
  drafts.discardGenerated(a)
  assert.equal(drafts.get(a).generated, null)

  const errors = [
    async () => { throw { kind: 'headless', message: 'not installed' } },
    async () => answer(' \n'),
    async () => ({ text: 'quota exceeded', isError: true, subtype: 'error' }),
  ]
  for (const run of errors) {
    await drafts.generate(a, run)
    assert.equal(drafts.get(a).message, 'latest edit')
    assert.equal(drafts.get(a).generating, false)
    assert.ok(drafts.get(a).error)
  }
  drafts.clearAfterCommit(a, oldRevision)
  assert.equal(drafts.get(a).message, 'latest edit')
  drafts.clearAfterCommit(a, drafts.get(a).revision)
  assert.equal(drafts.get(a).message, '')
  await drafts.generate(a, async () => answer('  clean draft \n'))
  assert.equal(drafts.get(a).message, 'clean draft')
  assert.equal(drafts.get(a).generated, null)

  // Mounted React/IPC integration, including a full unmount while the model is running.
  let current
  function Harness({ project, worktree }) {
    const git = useGitPanel(project, { worktree })
    current = git
    return React.createElement(GitPanelView, { git, project, iconTheme: 'dark' })
  }
  root = createRoot(document.querySelector('#root'))
  const mount = async (project = 'project-a', worktree = null) => {
    await act(async () => root.render(React.createElement(Harness, {
      key: `${project}:${worktree}`, project, worktree,
    })))
  }
  const close = async () => { await act(async () => root.render(null)) }
  const click = async (button) => {
    assert.ok(button, 'button exists')
    await act(async () => button.click())
  }
  const byText = (text) => [...document.querySelectorAll('button')]
    .find((button) => button.textContent === text)
  const generateButton = () => document.querySelector('[data-audit="gitGenerateMessage"]')
  const textarea = () => document.querySelector('[data-audit="gitMessage"]')
  const type = async (text) => {
    await act(async () => {
      Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, 'value')
        .set.call(textarea(), text)
      textarea().dispatchEvent(new dom.window.Event('input', { bubbles: true }))
    })
  }
  const pick = async (path) => {
    await act(async () => current.toggleCheck(current.rows.find((row) => row.entry?.path === path)))
  }

  await mount()
  assert.equal(generateButton().disabled, true)
  assert.ok(generateButton().title.includes('Select changes'))
  assert.equal(generateButton().textContent, '', 'generation uses a compact icon button')
  assert.equal(generateButton().querySelector('svg').dataset.icon, 'pencil')
  await type('manual draft')
  assert.equal(current.message, 'manual draft')
  await click(byText('Shelf'))
  await click(byText('Commit'))
  assert.equal(textarea().value, 'manual draft')
  await close()
  await mount()
  assert.equal(textarea().value, 'manual draft')

  await pick('picked.txt')
  await act(async () => setPartial({ repo: 'repo-a', path: 'picked.txt', side: 'combined',
    rev: 'rev-one', selection: { kind: 'lines', lines: [{ hunk: 0, line: 1 }] }, lines: 1 }))
  assert.equal(generateButton().title, 'Generate commit message')
  assert.equal(generateButton().getAttribute('aria-label'), generateButton().title)
  await click(generateButton())
  const generatedCall = calls.filter((call) => call.command === 'claude_commit_message').at(-1)
  assert.deepEqual(generatedCall.args.scopes, [{ repo: 'repo-a', selections: [{
    path: 'picked.txt', rev: 'rev-one', selection: { kind: 'lines', lines: [{ hunk: 0, line: 1 }] },
  }] }])
  assert.equal(generateButton().disabled, true)
  assert.equal(generateButton().querySelector('svg').dataset.icon, 'loader-circle')
  assert.equal(generateButton().getAttribute('aria-busy'), 'true')
  assert.equal(document.querySelector('[data-audit="gitCommit"]').disabled, true)
  assert.equal(textarea().disabled, false)
  await type('edited while generating')
  await close()
  await act(async () => generation.resolve(answer('generated after close')))
  await mount()
  assert.equal(textarea().value, 'edited while generating')
  assert.ok(document.querySelector('[data-audit="gitReplaceMessage"]'))
  assert.equal(document.activeElement.textContent, 'Cancel')
  await click(byText('Cancel'))
  assert.equal(textarea().value, 'edited while generating')
  assert.equal(document.querySelector('[data-audit="gitReplaceMessage"]'), null)

  generation = deferred()
  await click(generateButton())
  await act(async () => generation.resolve(answer('accepted generated draft')))
  await click(byText('Replace'))
  assert.equal(textarea().value, 'accepted generated draft')
  await mount('project-b')
  assert.equal(textarea().value, '')
  await type('project b draft')
  await mount('project-a', '/test/worktree')
  assert.equal(textarea().value, '')
  await type('worktree draft')
  await mount('project-a')
  assert.equal(textarea().value, 'accepted generated draft')

  // Failed commits retain text; successful commits clear only the submitted revision.
  await click(document.querySelector('[data-audit="gitCommit"]'))
  await act(async () => commit.reject({ kind: 'git', detail: 'commit failed' }))
  assert.equal(textarea().value, 'accepted generated draft')
  commit = deferred()
  await click(document.querySelector('[data-audit="gitCommit"]'))
  await type('draft for next commit')
  await close()
  await act(async () => commit.resolve({ oid: 'new-commit', files: 1 }))
  await mount()
  assert.equal(textarea().value, 'draft for next commit')
  commit = deferred()
  await click(document.querySelector('[data-audit="gitCommit"]'))
  await act(async () => commit.resolve({ oid: 'new-commit', files: 1 }))
  assert.equal(textarea().value, '')

  // A successful generation into an empty draft is inserted without confirmation.
  generation = deferred()
  await click(generateButton())
  await act(async () => generation.resolve(answer('automatic draft')))
  assert.equal(textarea().value, 'automatic draft')
  assert.equal(document.querySelector('[data-audit="gitReplaceMessage"]'), null)
  generation = deferred()
  await click(generateButton())
  await act(async () => generation.reject({ kind: 'headless', message: 'CLI unavailable' }))
  assert.equal(textarea().value, 'automatic draft')
  assert.ok(document.querySelector('[role="alert"]').textContent.includes('CLI unavailable'))

  // Unchecked repositories are excluded; staged mode still targets the same repo as Commit.
  await close()
  clearAllPartials()
  status = { repos: [repo('staged-repo', true), repo('untargeted-repo', true)] }
  await mount('staged-project')
  await pick('picked.txt')
  generation = deferred()
  await click(generateButton())
  const stagedCall = calls.filter((call) => call.command === 'claude_commit_message').at(-1)
  assert.equal(stagedCall.args.scopes.length, 1)
  assert.equal(stagedCall.args.scopes[0].repo, 'staged-repo')
  await act(async () => generation.resolve(answer('staged draft')))
  await close()
  status = { repos: [repo('reword-repo')] }
  await mount('reword-project')
  await act(async () => current.setAmend(true))
  assert.equal(generateButton().disabled, true, 'message-only amend has no changes')

  assert.equal(commitDrafts.get(commitDraftKey('project-b', null)).message, 'project b draft')
  assert.equal(commitDrafts.get(commitDraftKey('project-a', '/test/worktree')).message, 'worktree draft')
  console.log('Commit messages: scoped drafts, partial-selection IPC, async retention, confirmation and commit clearing passed')
} finally {
  if (root) await act(async () => root.unmount())
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
