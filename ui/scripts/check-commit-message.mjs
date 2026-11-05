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
/** What a walk of an agent checkout answers. A different tree from the project's. */
let checkoutStatus = null
const statusReads = []
const shelfReads = []
const calls = []
let generation = deferred()
let commit = deferred()
// Listeners by event id, so a test can deliver a broadcast the way Rust would. Unlisten is honoured:
// a panel from an earlier mount must not answer one, because the real one no longer would.
const callbacks = new Map()
const listeners = new Map()
let nextId = 1
const broadcast = (event, payload) => {
  for (const listener of listeners.values()) {
    if (listener.event === event) listener.handler({ event, id: 0, payload })
  }
}
window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} }
window.__TAURI_INTERNALS__ = {
  transformCallback: (callback) => {
    const id = nextId++
    callbacks.set(id, callback)
    return id
  },
  unregisterCallback: () => {},
  invoke: async (command, args) => {
    calls.push({ command, args })
    if (command === 'plugin:event|listen') {
      const id = nextId++
      listeners.set(id, { event: args.event, handler: callbacks.get(args.handler) })
      return id
    }
    if (command === 'plugin:event|unlisten') {
      listeners.delete(args.eventId)
      return
    }
    if (command === 'git_status') {
      return statusReads.shift()?.promise
        ?? structuredClone(args.worktree ? checkoutStatus ?? status : status)
    }
    if (command === 'git_conflicts') return null
    if (command === 'git_shelf_list') return shelfReads.shift()?.promise ?? []
    if (command === 'claude_commit_message') return generation.promise
    if (command === 'claude_commit_message_cancel') return true
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
    setPartial, clearAllPartials, useGitChangeCount, useGitCount,
  } = await import(`file://${resolve(out, 'commitSmoke.js')}`)

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
  pending = deferred()
  let reportProgress
  const progressRun = drafts.generate(b, (_id, report) => {
    reportProgress = report
    report('analyzing', 2, 5)
    return pending.promise
  }, () => { calls.push({ command: 'cancelled-store' }) })
  assert.equal(drafts.get(b).progress, 'Analyzing changes 2/5…')
  drafts.cancel(b)
  reportProgress('analyzing', 3, 5)
  assert.equal(drafts.get(b).progress, 'Cancelling generation…')
  assert.ok(calls.some(call => call.command === 'cancelled-store'))
  pending.reject({ kind: 'cancelled', message: 'cancelled' })
  await progressRun
  assert.equal(drafts.get(b).message, 'worktree draft')
  assert.equal(drafts.get(b).error, null)
  pending = deferred()
  let startedAfterCancel = false
  const earlyCancel = drafts.generate(b, async (_id, _progress, isCancelled) => {
    await pending.promise
    if (isCancelled()) throw { kind: 'cancelled', message: 'cancelled' }
    startedAfterCancel = true
    return answer('should not appear')
  })
  drafts.cancel(b)
  pending.resolve()
  await earlyCancel
  assert.equal(startedAfterCancel, false)
  assert.equal(drafts.get(b).error, null)
  drafts.setSelection(b, 'old-selection')
  pending = deferred()
  const staleSelection = drafts.generate(b, () => pending.promise, undefined, 'old-selection')
  drafts.setSelection(b, 'new-selection')
  pending.resolve(answer('stale result'))
  await staleSelection
  assert.equal(drafts.get(b).message, 'worktree draft')
  assert.ok(drafts.get(b).error.includes('selected files or hunks changed'))
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
  generation = deferred()
  await click(generateButton())
  const activeRequest = calls.filter(call => call.command === 'claude_commit_message').at(-1).args.requestId
  await click(byText('Cancel'))
  assert.equal(calls.filter(call => call.command === 'claude_commit_message_cancel').at(-1).args.requestId, activeRequest)
  await act(async () => generation.reject({ kind: 'cancelled', message: 'cancelled' }))
  assert.equal(textarea().value, 'accepted generated draft')
  assert.equal(current.draft.error, null)
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
  assert.ok(document.querySelector('button[aria-label="Cancel commit message generation"]'))
  assert.ok(document.querySelector('[data-audit="gitSummary"]').textContent.includes('Reading selected changes'))
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

  // A task removed outside Changes must be recoverable without losing the rest of a commit.
  await close()
  clearAllPartials()
  const taskPath = '.cide/tasks/t-1/task.json'
  status = { repos: [repo('refresh-repo')] }
  status.repos[0].changelists[0].changes.push(change(taskPath))
  await mount('refresh-project')
  await pick('picked.txt')
  await pick(taskPath)
  await type('keep this draft')
  const hold = (path) => setPartial({ repo: 'refresh-repo', path, side: 'combined',
    rev: `rev:${path}`, selection: { kind: 'lines', lines: [{ hunk: 0, line: 1 }] }, lines: 1 })
  await act(async () => { hold(taskPath); hold('picked.txt') })
  commit = deferred()
  await click(document.querySelector('[data-audit="gitCommit"]'))
  await act(async () => commit.reject({ kind: 'noSuchChange', detail: { path: taskPath } }))
  assert.ok(current.unavailable.includes(taskPath), 'automatic post-failure refresh keeps the error')
  const previousError = current.unavailable
  status.repos[0].changelists[0].changes = status.repos[0].changelists[0].changes
    .filter((entry) => entry.path !== taskPath)
  const refreshButton = () => document.querySelector('button[aria-label="Refresh changes"]')
  await click(refreshButton())
  assert.equal(current.unavailable, null, 'a successful manual refresh clears the preceding error')
  assert.equal(document.querySelector('[data-audit="gitUnavailable"]'), null)
  assert.equal(current.rows.some((row) => row.entry?.path === taskPath), false)
  assert.deepEqual(current.picked.map((entry) => entry.path), ['picked.txt'])
  assert.equal(current.selected.size, 1, 'obsolete ticks are pruned without checking another file')
  assert.deepEqual(current.partials.map((entry) => entry.path), ['picked.txt'])
  assert.equal(textarea().value, 'keep this draft')

  // A failed status read keeps the draft and held selections for the next good answer.
  let read = deferred()
  statusReads.push(read)
  await click(refreshButton())
  await act(async () => read.reject({ kind: 'git', detail: { detail: 'status locked' } }))
  assert.ok(current.unavailable.includes('status locked'))
  assert.notEqual(current.unavailable, previousError)
  assert.equal(textarea().value, 'keep this draft')
  assert.equal(current.selected.size, 1)
  assert.equal(current.partials.length, 1)
  await click(refreshButton())
  assert.equal(current.unavailable, null)
  assert.deepEqual(current.picked.map((entry) => entry.path), ['picked.txt'])

  // An older read cannot resurrect a task after a later refresh removed it.
  const older = deferred()
  const newer = deferred()
  statusReads.push(older, newer)
  await click(refreshButton())
  await click(refreshButton())
  await act(async () => newer.resolve(structuredClone(status)))
  const stale = structuredClone(status)
  stale.repos[0].changelists[0].changes.push(change(taskPath))
  await act(async () => older.resolve(stale))
  assert.equal(current.rows.some((row) => row.entry?.path === taskPath), false)

  const olderFailure = deferred()
  const latest = deferred()
  statusReads.push(olderFailure, latest)
  await click(refreshButton())
  await click(refreshButton())
  await act(async () => latest.resolve(structuredClone(status)))
  await act(async () => olderFailure.reject({ kind: 'git', detail: { detail: 'obsolete failure' } }))
  assert.equal(current.unavailable, null, 'an older failed read cannot replace a successful refresh')

  // An independent failure can arrive without superseding the pending status read.
  read = deferred()
  const shelfRead = deferred()
  statusReads.push(read)
  shelfReads.push(shelfRead)
  await click(refreshButton())
  await act(async () => shelfRead.reject({ kind: 'git', detail: { detail: 'new shelf failure' } }))
  const shelfError = current.unavailable
  assert.ok(shelfError.includes('new shelf failure'))
  await act(async () => read.resolve(structuredClone(status)))
  assert.equal(current.unavailable, shelfError, 'refresh acknowledges only the error it started with')
  await click(refreshButton())
  assert.equal(current.unavailable, null)

  // A refresh started before a new commit failure must not hide that newer error.
  read = deferred()
  statusReads.push(read)
  await click(refreshButton())
  commit = deferred()
  await click(document.querySelector('[data-audit="gitCommit"]'))
  const afterFailure = deferred()
  statusReads.push(afterFailure)
  await act(async () => commit.reject({ kind: 'git', detail: { detail: 'new commit failure' } }))
  const newerError = current.unavailable
  assert.ok(newerError.includes('new commit failure'))
  await act(async () => read.resolve(structuredClone(status)))
  assert.equal(current.unavailable, newerError)
  await act(async () => afterFailure.resolve(structuredClone(status)))
  assert.equal(current.unavailable, newerError, 'background refresh leaves the newer failure visible')
  assert.equal(textarea().value, 'keep this draft')
  await click(refreshButton())
  assert.equal(current.unavailable, null)

  /*
   * An answer that changed nothing installs nothing, which is what every rail switch back to an
   * unchanged tree relies on: the remount paints from the cache and this revalidation must not
   * rebuild the same rows underneath it.
   */
  const rowsBefore = current.rows
  await click(refreshButton())
  assert.equal(current.rows, rowsBefore, 'an identical status keeps the rows, by identity')

  /*
   * And that shortcut must not compare against the wrong thing. Both broadcasts land in one `act`,
   * so React renders nothing in between: the second is newest and equal to what is on screen, while
   * the first is queued and not yet drawn. Measured against the *rendered* view the newest answer
   * reads as no change and is dropped, the panel settles on the older tree, and if nothing else
   * touches the working copy nothing ever corrects it.
   */
  const between = structuredClone(status)
  between.repos[0].changelists[0].changes.push(change('landed-between.txt'))
  const hasBetween = () => current.rows.some((row) => row.entry?.path === 'landed-between.txt')
  // The control. The assertion below is about a row being absent, which would also hold if a
  // broadcast never reached the panel at all.
  await act(async () => broadcast('cide://git-status', { project: 'refresh-project', tree: between }))
  assert.equal(hasBetween(), true, 'a broadcast reaches the panel and draws its row')
  await act(async () => {
    broadcast('cide://git-status', { project: 'refresh-project', tree: structuredClone(status) })
  })
  assert.equal(hasBetween(), false, 'and the next one takes it away again')
  await act(async () => {
    broadcast('cide://git-status', { project: 'refresh-project', tree: between })
    broadcast('cide://git-status', { project: 'refresh-project', tree: structuredClone(status) })
  })
  assert.equal(
    hasBetween(),
    false,
    'the newest of two answers wins, even when it equals what was on screen before either arrived',
  )

  commit = deferred()
  await click(document.querySelector('[data-audit="gitCommit"]'))
  const refreshedCommit = calls.filter((call) => call.command === 'git_commit').at(-1)
  assert.deepEqual(refreshedCommit.args.request.selections.map((entry) => entry.path), ['picked.txt'])
  await act(async () => commit.resolve({ oid: 'after-refresh', files: 1 }))
  assert.equal(textarea().value, '', 'the remaining checked changes can be committed')

  assert.equal(commitDrafts.get(commitDraftKey('project-b', null)).message, 'project b draft')
  assert.equal(commitDrafts.get(commitDraftKey('project-a', '/test/worktree')).message, 'worktree draft')

  /*
   * The rail's badge and an open panel do not both walk for one burst, and the badge is right
   * either way.
   *
   * The second half is the one that matters: dropping a walk is only safe where something else is
   * answering, and each case below is one where a simpler version of this leaves the badge showing
   * a wrong number with nothing due to correct it. Real timers, because the 120 ms throttles are
   * the thing under test, on both sides.
   */
  const badge = 'badge-project'
  const treeOf = (files) => ({ repos: [{
    ...repo('badge-repo'),
    changelists: [{ id: 'default', name: 'Changes', active: true, comment: '',
      changes: Array.from({ length: files }, (_, i) => change(`f${i}.txt`)) }],
  }] })
  function Badged({ worktree, panel, panelProject }) {
    useGitChangeCount(badge)
    return panel
      ? React.createElement(Harness, {
        key: `${panelProject}:${worktree}`, project: panelProject, worktree,
      })
      : null
  }
  const show = (panel, worktree = null, panelProject = badge) => act(async () => root.render(
    React.createElement(Badged, { panel, worktree, panelProject }),
  ))
  const settle = () => act(async () => { await new Promise((done) => setTimeout(done, 200)) })
  const burst = (event = 'cide://fs-changed') => act(async () => {
    broadcast(event, event === 'cide://fs-changed'
      ? { project: badge, change: {} }
      : { session: 'some-session', paths: [] })
  })
  const walks = () => calls.filter(
    (call) => call.command === 'git_status' && call.args.project === badge,
  ).length
  const badgeCount = () => useGitCount.getState().count
  const walksFor = async (trigger) => {
    const before = walks()
    await trigger()
    await settle()
    return walks() - before
  }

  // Panel shut: the badge is on its own, so it walks.
  await close()
  status = treeOf(3)
  await show(false)
  await settle()
  assert.equal(badgeCount(), 3, 'the badge counts the project with no panel open')
  assert.equal(await walksFor(() => burst()), 1, 'and a burst is one walk, which is its own')

  // Panel open on the project's tree: one walk per burst, whichever signal it arrives on.
  await show(true)
  await settle()
  status = treeOf(5)
  assert.equal(await walksFor(() => burst()), 1, 'a file burst costs one walk with the panel open')
  assert.equal(badgeCount(), 5, 'and the badge moved anyway, off the walk the panel made')
  status = treeOf(6)
  assert.equal(
    await walksFor(() => burst('cide://session-tool')),
    1,
    'a tool call costs one walk too: the badge subscribes to both signals, and so does the panel',
  )
  assert.equal(badgeCount(), 6)
  // *Refresh* asks by name. A held count must not make that command quietly do nothing.
  assert.equal(
    await walksFor(async () => { await act(async () => useGitCount.getState().refresh()) }),
    1,
    'an explicit refresh still walks',
  )

  // A burst the panel never got to answer: its timer is cancelled by the unmount, and the store
  // skipped the burst because the panel was open. Without a walk on release nobody answers it.
  status = treeOf(7)
  await burst()
  await show(false)
  await settle()
  assert.equal(badgeCount(), 7, 'closing the panel catches the badge up on what it had skipped')
  assert.equal(await walksFor(() => burst()), 1, 'and it is back to walking for itself')

  // A panel on an agent checkout shows a different tree, and the badge counts the project.
  status = treeOf(4)
  checkoutStatus = treeOf(2)
  await show(true, '/checkout')
  await settle()
  await act(async () => useGitCount.getState().refresh())
  assert.equal(badgeCount(), 4, 'the project\'s count, not the two files of the checkout on screen')
  assert.equal(current.rows.filter((row) => row.kind === 'file').length, 2, 'which is what it shows')
  // Tagged with the checkout, so no walk of the badge's follows to paper over a wrong write.
  await act(async () => {
    broadcast('cide://git-status', { project: badge, tree: treeOf(9), worktree: '/checkout' })
  })
  assert.equal(current.rows.filter((row) => row.kind === 'file').length, 9, 'the panel took that')
  assert.equal(badgeCount(), 4, 'and the badge did not')
  status = treeOf(8)
  assert.equal(
    await walksFor(() => burst()),
    2,
    'nobody is feeding the project\'s count from a checkout, so here both walk: one tree each',
  )
  assert.equal(badgeCount(), 8, 'and the badge follows the project')
  checkoutStatus = null

  // A panel on some other project (a detached window's) feeds nothing here, so it holds nothing.
  await show(true, null, 'other-project')
  await settle()
  status = treeOf(11)
  assert.equal(
    await walksFor(() => burst()),
    1,
    'a panel on another project does not stop this project\'s badge from walking',
  )
  assert.equal(badgeCount(), 11, 'and that walk is what keeps it right')
  await close()

  console.log('Commit panel: scoped drafts, partial selections, generation, refresh recovery, commit clearing and the badge\'s walks passed')
} finally {
  if (root) await act(async () => root.unmount())
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
