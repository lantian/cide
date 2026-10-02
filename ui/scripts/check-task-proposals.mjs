/** Exercise the real create store and queued-tab follower with IPC and workspace boundaries
 * replaced. Saving, launching and presentation can fail independently; a source-text check
 * cannot show that a saved task closes its draft while a rejected save keeps it. */
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { resolve, join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

const ui = resolve(import.meta.dirname, '..')
mkdirSync(join(ui, 'node_modules/.cache'), { recursive: true })
const out = mkdtempSync(join(ui, 'node_modules/.cache/task-proposals-'))
const listeners = new Set()
const calls = { create: [], propose: [], apply: [], opened: [], failures: [], specTabs: [], focusedTabs: [] }
let rows = []
let saveError = null
let launchError = null
let openPlan = null
let readError = null
let applyResult = null
const board = { kind: 'ready', tasks: [], rev: 1 }
globalThis.__taskProposalChecks = {
  tasks: {
    create: async (req) => { calls.create.push(req); if (saveError) throw saveError; return board },
    createWithProposal: async (req) => {
      calls.propose.push(req)
      if (saveError) throw saveError
      return { board, task: 't-42', proposal: launchError ? null : 'run-42', proposalError: launchError }
    },
  },
  specSessions: {
    start: async (...args) => { calls.apply.push(args); return applyResult ?? 'apply-42' },
    onChanged: async (handler) => { listeners.add(handler); return () => listeners.delete(handler) },
    runs: async () => { if (readError) throw readError; return rows },
  },
  agentRuns: { open: async () => openPlan ?? { kind: 'live', session: 'session-42', continues: null } },
  spec: { openTab: async (...args) => { calls.specTabs.push(args); return 'spec-tab-42' } },
  opened: calls.opened,
  focusedTabs: calls.focusedTabs,
  failures: calls.failures,
}

try {
  await build({
    configFile: false,
    root: ui,
    logLevel: 'error',
    resolve: { alias: { '@': join(ui, 'src') } },
    plugins: [{
      name: 'task-proposal-check-boundaries', enforce: 'pre',
      resolveId(id) {
        if (id.endsWith('virtual:task-proposals')) return '\0checks'
        if (id === '@/ipc/client' || id === join(ui, 'src/ipc/client')) return '\0client'
        if (id === '@/store/workspace' || id === join(ui, 'src/store/workspace')) return '\0workspace'
        if (id === '@/chrome/notices' || id === join(ui, 'src/chrome/notices')) return '\0notices'
      },
      load(id) {
        if (id === '\0checks') return `
          export { useTasks } from ${JSON.stringify(join(ui, 'src/sidebar/tasksStore.ts'))};
          export { openTaskSpec, approveTaskSpec } from ${JSON.stringify(join(ui, 'src/sidebar/TasksPanel/openSpec.ts'))};
          export { TaskDetail } from ${JSON.stringify(join(ui, 'src/sidebar/TasksPanel/TaskDetail.tsx'))};
          export { CARD_STORIES } from ${JSON.stringify(join(ui, 'src/sidebar/TasksPanel/fixture.ts'))};
          export { taskSpecSessions, taskSpecStatuses, taskSpecPhase } from ${JSON.stringify(join(ui, 'src/sidebar/TasksPanel/specSessions.ts'))};
          export { EMPTY_DRAFT } from ${JSON.stringify(join(ui, 'src/sidebar/TasksPanel/model.ts'))};
          export { followRunTab, followSpecRuns, useSpecRuns, openSession } from ${JSON.stringify(join(ui, 'src/sidebar/OpenSpecPanel/specRuns.ts'))};
          export { useWorkspace } from '@/store/workspace';`
        if (id === '\0client') return `
          const t = globalThis.__taskProposalChecks;
          export const tasks = t.tasks, attachments = {}, specEvents = {};
          export const settings = { effective: async (project) => ({ consoleHarness: t.projectHarness?.[project] ?? t.consoleHarness ?? 'codex' }) };
          export const specSessions = t.specSessions, agentRuns = t.agentRuns, spec = t.spec;`
        if (id === '\0workspace') return `
          import { create } from 'zustand';
          export const useWorkspace = create(() => ({ boot: null,
            newRunTab: async (...args) => globalThis.__taskProposalChecks.opened.push(args),
            activateTab: async (...args) => globalThis.__taskProposalChecks.focusedTabs.push(args) }));`
        if (id === '\0notices') return `
          export const notifyFailure = (...args) => globalThis.__taskProposalChecks.failures.push(args);`
      },
    }],
    build: { ssr: 'virtual:task-proposals', outDir: out, rollupOptions: { output: { entryFileNames: 'checks.mjs' } } },
  })
  const { useTasks, useWorkspace, EMPTY_DRAFT, followRunTab, followSpecRuns, useSpecRuns, openTaskSpec, approveTaskSpec, TaskDetail, CARD_STORIES, taskSpecSessions, taskSpecStatuses, taskSpecPhase, openSession } = await import(pathToFileURL(join(out, 'checks.mjs')).href)
  const workspace = (active = 'project-1', mode = 'background', harness = 'codex') => { globalThis.__taskProposalChecks.consoleHarness = harness; return useWorkspace.setState({
    boot: { role: { kind: 'shell', active }, workspace: { settings: { taskProposalRunMode: mode, consoleHarness: harness } } },
  }) }
  const draft = { ...EMPTY_DRAFT, title: ' Dark mode ', propose: true, assignee: 'developer' }
  const reset = (mode = 'background') => {
    workspace('project-1', mode)
    useTasks.setState({ project: 'project-1', board: { kind: 'absent', path: '/repo' }, compose: draft, creating: false })
    rows = [{ run: 'run-42', state: { state: 'queued' } }]
    saveError = launchError = readError = openPlan = null
    applyResult = null
    for (const values of Object.values(calls)) values.length = 0
  }
  const flush = async () => { for (let i = 0; i < 8; i++) await new Promise(setImmediate) }
  const emit = (next) => {
    rows = next
    for (const handler of [...listeners]) handler('project-1', rows)
  }

  reset()
  await useTasks.getState().create({ ...draft, propose: false })
  assert.equal(calls.create.length, 1)
  assert.equal(calls.propose.length, 0, 'ordinary creates never request a proposal')

  reset()
  await useTasks.getState().create(draft)
  await flush()
  assert.equal(calls.propose.length, 1)
  assert.equal(calls.propose[0].agent, 'developer', 'assignment is preserved')
  assert.equal(calls.propose[0].change, undefined, 'no invented or sentinel change reaches Rust')
  assert.equal(useTasks.getState().compose, null, 'saved draft closes')
  assert.equal(listeners.size, 0, 'background proposals do not follow an automatic tab')
  assert.equal(calls.opened.length, 0)

  reset()
  launchError = 'No propose workflow installed'
  await useTasks.getState().create(draft)
  assert.equal(useTasks.getState().compose, null, 'launch failure retains the saved task, not a retryable draft')
  assert.equal(calls.failures.length, 1)
  assert.match(calls.failures[0][0], /t-42 was saved/)

  reset()
  saveError = new Error('Attachment went away')
  await assert.rejects(useTasks.getState().create(draft), /Attachment went away/)
  assert.equal(useTasks.getState().compose, draft, 'failed save keeps all draft fields')
  assert.equal(useTasks.getState().creating, false)
  assert.equal(listeners.size, 0)

  reset('tab')
  await useTasks.getState().create(draft)
  await flush()
  assert.equal(listeners.size, 1, 'queued run is followed without a timeout')
  followRunTab('project-1', 'run-42', 'Apply')
  assert.equal(listeners.size, 1, 'duplicate follow requests do not open two tabs')
  emit([{ run: 'run-42', session: 'session-42', state: { state: 'running' } }])
  await flush()
  assert.equal(calls.opened.length, 1)
  assert.equal(listeners.size, 0, 'follower is released after opening')

  reset('tab')
  followRunTab('project-1', 'run-42', 'Apply')
  await flush()
  workspace('another-project')
  emit([{ run: 'run-42', state: { state: 'running' } }])
  await flush()
  assert.equal(calls.opened.length, 0, 'queued run never steals focus after a project switch')
  assert.equal(listeners.size, 0)

  reset('tab')
  let answerOpen
  openPlan = new Promise((resolve) => { answerOpen = resolve })
  followRunTab('project-1', 'run-42', 'Apply')
  await flush()
  emit([{ run: 'run-42', state: { state: 'running' } }])
  workspace('another-project')
  answerOpen({ kind: 'live', session: 'session-42', continues: null })
  await flush()
  assert.equal(calls.opened.length, 0, 'project switch during the IPC open request also cancels presentation')

  reset('tab')
  followRunTab('project-1', 'run-42', 'Apply')
  await flush()
  emit([{ run: 'run-42', state: { state: 'failed', reason: 'Harness unavailable' } }])
  await flush()
  assert.equal(calls.opened.length, 0)
  assert.equal(calls.failures.length, 1)
  assert.equal(listeners.size, 0)

  reset('tab')
  readError = new Error('Run listing failed')
  followRunTab('project-1', 'run-42', 'Apply')
  await flush()
  assert.equal(listeners.size, 0, 'failed initial read releases both subscriptions')
  assert.equal(calls.failures.length, 1)
  reset()
  rows = [{ run: 'run-42', task: 't-42', op: 'propose', state: { state: 'queued' } }]
  const stopCard = followSpecRuns('project-1', false)
  await flush()
  assert.equal(useSpecRuns.getState().runs[0].task, 't-42', 'the task card receives the persisted proposal association')
  emit([{ ...rows[0], session: 'session-42', state: { state: 'running' } }])
  await flush()
  assert.equal(useSpecRuns.getState().runs[0].session, 'session-42', 'the card link updates when a queued session starts')
  assert.equal(calls.opened.length, 0, 'following task links never opens a background run automatically')
  stopCard()
  await flush()
  assert.equal(listeners.size, 0, 'task-card proposal subscriptions are released')
  reset()
  useTasks.getState().select('t-42')
  let answerSpecTab
  globalThis.__taskProposalChecks.spec.openTab = async (...args) => {
    calls.specTabs.push(args)
    return new Promise((resolve) => { answerSpecTab = resolve })
  }
  const navigation = openTaskSpec('project-1', 'add-dark-mode')
  assert.equal(useTasks.getState().selected, null, 'spec navigation closes the task modal before the tab request finishes')
  assert.deepEqual(calls.specTabs, [['project-1', { kind: 'change', change: 'add-dark-mode' }]], 'both card controls open the linked change subject')
  assert.equal(calls.focusedTabs.length, 0, 'focus waits for the tab to exist')
  answerSpecTab('spec-tab-42')
  await navigation
  assert.deepEqual(calls.focusedTabs, [['project-1', 'spec-tab-42']], 'navigation explicitly focuses and synchronizes the opened or reused tab')
  reset()
  const apply = { run: 'apply-42', op: 'apply', task: null, change: 'add-dark-mode', label: 'OpenSpec · Codex', state: { state: 'running' }, session: 'apply-session-42' }
  const workflows = [
    { run: 'proposal-42', op: 'propose', task: 't-42', change: null },
    apply,
    { ...apply, run: 'role-apply', task: 't-43' },
    { ...apply, run: 'unlinked-task-apply', task: 't-42', change: null },
    { ...apply, run: 'unrelated', change: 'another-change' },
    { ...apply, run: 'explore', op: 'explore' },
    { run: 'unrelated-proposal', op: 'propose', task: 't-43', change: 'add-dark-mode' },
  ]
  assert.deepEqual(taskSpecSessions(workflows, 't-42', 'add-dark-mode').map((row) => row.run), ['proposal-42', 'apply-42', 'role-apply', 'unlinked-task-apply'], 'Apply links by change across standalone and role runs; proposals link by origin task')
  assert.deepEqual(taskSpecSessions(workflows, 't-42', null).map((row) => row.run), ['proposal-42', 'unlinked-task-apply'], 'an absent change never matches unrelated standalone Apply')
  assert.deepEqual(taskSpecSessions(workflows, null, 'add-dark-mode'), [], 'there are no session links without a selected task')
  await openSession('project-1', apply)
  assert.equal(calls.opened.length, 1, 'Apply opens through the same live-run mirror path as proposals')
  assert.equal(calls.opened[0][0], 'project-1')
  assert.match(calls.opened[0][1], /Apply.*add-dark-mode/)
  const summaryRuns = [
    { ...apply, run: 'finished-apply', state: { state: 'finished', code: 0 } },
    { ...apply, run: 'queued-apply', state: { state: 'queued' } },
    { ...apply, run: 'running-apply' },
    { ...apply, run: 'waiting-apply', state: { state: 'awaitingPermission' } },
    { ...apply, run: 'old-proposal', op: 'propose', task: 't-42', state: { state: 'interrupted' } },
    { ...apply, run: 'queued-proposal', op: 'propose', task: 't-42', state: { state: 'queued' } },
  ]
  const unchanged = JSON.stringify(summaryRuns)
  assert.deepEqual(taskSpecStatuses(summaryRuns, 't-42', 'add-dark-mode').map((row) => row.run), ['queued-proposal', 'waiting-apply'], 'a current run wins over history and the most demanding live phase is shown for each workflow')
  assert.equal(JSON.stringify(summaryRuns), unchanged, 'summary never mutates the registry or subagent inputs')
  assert.deepEqual(taskSpecStatuses([summaryRuns[0]], 't-42', 'add-dark-mode').map((row) => row.run), ['finished-apply'], 'the latest ended status remains visible once work finishes')
  assert.deepEqual(taskSpecStatuses(summaryRuns, 'unrelated-task', null), [], 'unrelated rows gain no OpenSpec indicators')
  for (const state of ['idle', 'interrupted']) {
    const completed = { ...summaryRuns[4], state: { state }, turnComplete: true }
    assert.equal(taskSpecPhase(completed), 'finished', 'normal hand-back is shown as Finished before and after restart')
    assert.deepEqual(taskSpecStatuses([completed, summaryRuns[5]], 't-42', null).map((row) => row.run), ['queued-proposal'], 'a completed TUI turn does not hide a newer queued proposal')
    assert.equal(completed.state.state, state, 'workflow presentation leaves the real session lifecycle unchanged')
  }
  assert.equal(taskSpecPhase(summaryRuns[4]), 'interrupted', 'a mid-turn interruption remains Interrupted')

  // The primary button must invoke approval directly even with no roles or target sessions.
  const dom = new JSDOM('<!doctype html><div id="root"></div>', { url: 'https://cide.test' })
  for (const name of ['window', 'document', 'HTMLElement', 'Element', 'Node', 'DOMParser']) globalThis[name] = dom.window[name]
  globalThis.IS_REACT_ACT_ENVIRONMENT = true
  const { createRoot } = await import('react-dom/client')
  const root = createRoot(document.getElementById('root'))
  try {
    reset()
    useTasks.setState({ selected: 't-42' })
    rows = [{ run: 'apply-42', state: { state: 'queued' } }]
    let approval
    await act(async () => {
      root.render(React.createElement(TaskDetail, {
        ...CARD_STORIES['card-spec-ready'],
        task: { ...CARD_STORIES['card-spec-ready'].task, id: 't-42' },
        dispatchTargets: [],
        onDispatchOpen: () => assert.fail('approval must not open the empty target picker'),
        onSpecPrimary: (task, action) => {
          assert.equal(action, 'approve')
          approval = approveTaskSpec('project-1', task, 'add-dark-mode')
        },
      }))
    })
    await act(async () => {
      document.querySelector('[data-audit="specPrimary"]').click()
      await approval
      await flush()
    })
    assert.deepEqual(calls.apply, [['project-1', { op: 'apply', change: 'add-dark-mode', launcher: { kind: 'harness', harness: 'codex' } }]], 'one click starts Apply using the configured default harness')
    assert.equal(useTasks.getState().selected, null, 'successful approval closes the task modal')
    assert.equal(document.querySelector('[data-audit="taskDispatchTargets"]'), null, 'approval draws no empty target row')
    assert.equal(listeners.size, 1, 'a queued Apply is followed until admission')
    emit([{ run: 'apply-42', state: { state: 'running' }, session: 'apply-session' }])
    await flush()
    assert.equal(calls.opened.length, 1, 'explicit approval opens a tab even with background proposals enabled')
    assert.match(calls.opened[0][1], /Apply.*add-dark-mode.*codex/)
    assert.equal(listeners.size, 0)

    reset()
    workspace('project-1', 'background', 'claude')
    useTasks.setState({ selected: 't-42' })
    let finishLaunch
    applyResult = new Promise((resolve) => { finishLaunch = resolve })
    const pending = approveTaskSpec('project-1', 't-42', 'add-dark-mode')
    assert.equal(useTasks.getState().selected, 't-42', 'the modal remains while launch is pending')
    assert.equal(calls.opened.length, 0, 'no tab is opened before the launcher accepts')
    finishLaunch('apply-42')
    await pending
    await flush()
    assert.equal(calls.apply[0][1].launcher.harness, 'claude', 'Claude also follows the default setting')
    workspace('another-project')
    await flush()

    reset()
    useTasks.setState({ selected: 't-42' })
    applyResult = Promise.reject(new Error('Apply workflow missing'))
    await assert.rejects(approveTaskSpec('project-1', 't-42', 'add-dark-mode'), /Apply workflow missing/)
    assert.equal(useTasks.getState().selected, 't-42', 'a refused launch leaves the task card open')
    assert.equal(calls.opened.length, 0)
    assert.equal(listeners.size, 0)
  } finally {
    await act(async () => root.unmount())
    dom.window.close()
  }
  console.log('task proposals: ok (creation, background, queues, task session links, additive list statuses, spec navigation, approval click, cleanup)')
} finally {
  delete globalThis.__taskProposalChecks
  rmSync(out, { recursive: true, force: true })
}
