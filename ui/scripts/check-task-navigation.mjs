/** Mount the real task host, modal, Waiting origin and task store. Destinations take focus
 * immediately and can resolve later: the old bug left a modal above that focused terminal. */
import assert from 'node:assert/strict'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build } from 'vite'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'
import { flushSync } from 'react-dom'

const ui = resolve(import.meta.dirname, '..')
mkdirSync(join(ui, 'node_modules/.cache'), { recursive: true })
const out = mkdtempSync(join(ui, 'node_modules/.cache/task-navigation-'))
const dom = new JSDOM('<!doctype html><button id="agent">Agent terminal</button><div id="root"></div>', { url: 'https://cide.test', pretendToBeVisual: true })
for (const key of ['window', 'document', 'HTMLElement', 'Element', 'Node', 'MutationObserver']) globalThis[key] = dom.window[key]
globalThis.IS_REACT_ACT_ENVIRONMENT = true
dom.window.HTMLElement.prototype.scrollIntoView = function () {}
const { createRoot } = await import('react-dom/client')
const fixture = globalThis.taskNavigationCheck = { failures: [], navigations: [], task: null, milestone: null, navigate: () => Promise.resolve() }
const stores = `import { create } from 'zustand';
  export const useAgents = create(() => ({ roster: { kind: 'ready', agents: [], runs: [] },
    openPane: id => globalThis.taskNavigationCheck.navigate('run', id), pause: async () => {}, resume: async () => {} }));
  export const useWorkspace = create(() => ({ boot: null, addRow: async (...args) => {
    await globalThis.taskNavigationCheck.navigate('new-session', ...args); return { pane: 'pane' };
  }, activateTab: async () => {} }));
  export const useSpec = create(() => ({ board: { kind: 'ready', commands: [{ name: 'propose' }] } }));
  export const useMilestones = create(() => ({ view: null, reveal: id => { void globalThis.taskNavigationCheck.navigate('milestone', id) } }));
  export const followMilestones = () => () => {};
  export const milestoneOfTask = () => globalThis.taskNavigationCheck.milestone;
  export const useSpecRuns = create(() => ({ project: 'p', runs: [] }));
  export const followRunTab = () => {};
  export const openSession = (project, row) => globalThis.taskNavigationCheck.navigate('workflow', row.run);
  export const startSession = async (project, request, beforeOpen) => {
    await globalThis.taskNavigationCheck.launch(); beforeOpen?.();
    await globalThis.taskNavigationCheck.navigate('approve', request.change);
  };`
const mocks = {
  stores,
  ipc: `const f = () => globalThis.taskNavigationCheck;
    export const tasks = { get: async (_project, id) => f().read ? f().read(id) : f().board.tasks.find(task => task.id === id),
      edit: async (...args) => { f().edits.push(args); return f().board }, dispatchStatus: async () => null };
    export const claudeSend = { names: async () => ({}) };
    export const file = { open: (...args) => f().navigate('file', ...args) };
    export const attachments = { open: (...args) => f().navigate('attachment', ...args), reveal: (...args) => f().navigate('reveal', ...args) };
    export const milestones = { checkLog: async () => 'verification output' };
    export const settings = { effective: async () => ({ consoleHarness: 'codex' }) };
    export const spec = { change: async () => f().change, openTab: (...args) => f().navigate('spec', ...args),
      dispatchToSession: async () => {}, proposeForTask: async () => 'conversation' };
    export const events = {};`,
  notices: `export const notify = () => {}; export const notifyFailure = error => globalThis.taskNavigationCheck.failures.push(String(error));`,
  adapt: `export const adaptDetail = x => x; export const adaptBoard = x => x;`,
  specAdapt: `export const adaptChange = x => x;`,
  awaiting: `export const useAwaiting = () => false;`,
  previews: `export const previewsSnapshot = () => globalThis.taskNavigationCheck.previews;
    export const subscribePreviews = () => () => {}; export const requestPreview = () => {}; export const forgetPreview = () => {};`,
  drop: `export const onFileDrop = () => () => {}; export const useDropHot = () => null;`,
  session: `export const openTaskSession = (...args) => globalThis.taskNavigationCheck.navigate('session', ...args);`,
  entry: `export { TaskDetailHost } from '@/sidebar/TasksPanel/TaskDetailHost';
    export { CARD_STORIES } from '@/sidebar/TasksPanel/fixture';
    export { WaitingPanel } from '@/sidebar/AgentsPanel/WaitingPanel';
    export { useTasks } from '@/sidebar/tasksStore';
    export { leaveTaskDetails } from '@/sidebar/TasksPanel/taskNavigation';
    export { approveTaskSpec } from '@/sidebar/TasksPanel/openSpec';
    export { useAgents, useWorkspace, useSpecRuns, useMilestones } from 'navigation-check:stores';`,
}
let root
try {
  const entry = join(out, 'entry.ts')
  writeFileSync(entry, mocks.entry)
  await build({ root: ui, configFile: false, logLevel: 'error',
    resolve: { alias: { '@': join(ui, 'src') } },
    plugins: [{ name: 'task-navigation-fixture', enforce: 'pre',
      resolveId(source, importer) {
        if (source.startsWith('navigation-check:')) return '\0' + source
        let path = source
        if (source.startsWith('.') && importer) path = resolve(importer, '..', source)
        const is = (suffix) => path === join(ui, 'src', suffix) || path === join(ui, 'src', suffix + '.ts')
        const mock = is('ipc/client') ? 'ipc' : is('chrome/notices') ? 'notices'
          : ['sidebar/agentsStore', 'store/workspace', 'sidebar/specStore', 'sidebar/milestonesStore', 'sidebar/OpenSpecPanel/specRuns'].some(is) ? 'stores'
          : is('sidebar/TasksPanel/adapt') ? 'adapt' : is('sidebar/OpenSpecPanel/adapt') ? 'specAdapt'
          : is('panes/awaiting') ? 'awaiting' : is('sidebar/TasksPanel/attachmentPreviews') ? 'previews'
          : is('sidebar/TasksPanel/fileDrop') ? 'drop' : is('sidebar/TasksPanel/openSession') ? 'session' : null
        return mock === null ? null : '\0navigation-check:' + mock
      },
      load(id) { return id.startsWith('\0navigation-check:') ? mocks[id.slice('\0navigation-check:'.length)] : null },
    }],
    build: { ssr: entry, outDir: out, emptyOutDir: false, rollupOptions: { output: { entryFileNames: 'check.mjs' } } },
  })
  const { TaskDetailHost, CARD_STORIES, WaitingPanel, useTasks, useAgents, useWorkspace, useSpecRuns, useMilestones, leaveTaskDetails, approveTaskSpec } = await import(pathToFileURL(join(out, 'check.mjs')))
  const agent = document.getElementById('agent')
  const commit = (fn) => act(() => flushSync(fn))
  const click = async (selector) => {
    const button = document.querySelector(selector)
    assert.ok(button, `missing control ${selector}`)
    assert.ok(!button.disabled, `disabled control ${selector}`)
    button.focus()
    await act(async () => button.click())
  }
  const visibleModals = () => [...document.querySelectorAll('[aria-modal="true"]')].filter((el) => !el.closest('[inert], [hidden]'))
  const escape = async () => act(async () => document.activeElement.dispatchEvent(new dom.window.KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true })))
  function Host({ waiting = false }) {
    const selected = useTasks((s) => s.selected)
    const trail = useTasks((s) => s.decisionBackStack)
    const epoch = useTasks((s) => s.decisionEpoch)
    return React.createElement(React.Fragment, null,
      waiting && React.createElement(WaitingPanel, { key: epoch, project: 'p', title: 'Waiting', review: [], questions: [], reports: {}, busy: new Set(),
        requests: [{ id: 'decision', title: 'Check linked task', origin: { id: 'conversation', title: 'Agent' }, request: { kind: 'question', question: 'Which task?' }, createdUnixMs: 1 }],
        taskOpen: selected !== null && trail.length > 0, onOpenTask: (id) => useTasks.getState().selectFromDecision(id), onAccept() {}, onSendBack() {}, onAnswer() {},
      }),
      selected !== null && React.createElement(TaskDetailHost))
  }
  root = createRoot(document.getElementById('root'))
  let pending
  const configure = async (story = 'card-with-live-run', { waiting = false, nested = false, closedSession = false, milestone = false, verify = false, image = false, pendingRead = false } = {}) => {
    commit(() => root.render(null))
    const card = CARD_STORIES[story]
    fixture.task = structuredClone(card.task)
    fixture.task.attachments = [{ id: 'file-1', name: 'report.txt', kind: 'file', bytes: 10, target: { kind: 'task' } }]
    if (image) fixture.task.attachments.push({ id: 'image-1', name: 'image.png', kind: 'image', bytes: 10, target: { kind: 'task' } })
    fixture.change = { name: fixture.task.change, completed: 1, total: 2, validation: { valid: true, issues: [] }, tasks: [], deltas: [],
      artifacts: [{ id: 'proposal', existing: ['openspec/changes/test/proposal.md'] }, { id: 'tasks', existing: ['openspec/changes/test/tasks.md'] }] }
    fixture.board = { kind: 'ready', rev: 1, tasks: [fixture.task, { ...fixture.task, id: 'previous' }], path: '/project/.cide/tasks.json' }
    fixture.failures = []; fixture.navigations = []; fixture.edits = []
    fixture.read = pendingRead ? () => new Promise(() => {}) : null
    fixture.previews = image ? { 'image-1': { kind: 'ready', url: 'https://cide.test/image.png' } } : {}
    fixture.milestone = milestone ? { id: 'm-1', title: 'Milestone', state: 'active', goal: false } : null
    pending = null
    fixture.navigate = (kind, ...args) => {
      assert.equal(visibleModals().length, 0, `${kind}: close all overlays before the destination takes focus`)
      assert.equal(useTasks.getState().selected, null, `${kind}: no selected task`)
      assert.deepEqual(useTasks.getState().decisionBackStack, [], `${kind}: no previous card to resurrect`)
      fixture.navigations.push([kind, ...args])
      agent.focus()
      return new Promise((resolve, reject) => { pending = { resolve, reject } })
    }
    const session = fixture.task.session ?? 'conversation'
    const panes = closedSession ? {} : { pane: { id: 'pane', kind: 'claude', session, conversation: session } }
    useWorkspace.setState({ boot: { workspace: { projects: { p: { tabs: [{ id: 'console', tree: { panes } }], detached: {} } } } } })
    useAgents.setState({ roster: { kind: 'ready', agents: [{ id: 'developer', label: 'Developer', description: '', worktree: true }], runs: card.runs ?? [] } })
    useSpecRuns.setState({ project: 'p', runs: (card.workflowSessions ?? []).map((row) => ({ run: row.run, op: row.op, label: row.label,
      task: fixture.task.id, change: fixture.task.change, state: { state: 'running' }, turnComplete: false })) })
    useMilestones.setState({ view: verify ? { plan: { items: [] }, verifies: [{ task: fixture.task.id, running: false }] } : null })
    useTasks.setState({ project: 'p', board: fixture.board, selected: waiting ? null : fixture.task.id,
      decisionBackStack: nested ? [null, 'previous'] : [], dispatchAgent: null })
    commit(() => root.render(React.createElement(Host, { waiting })))
    if (waiting) {
      await click('[data-audit="waitingOpen"]')
      commit(() => useTasks.getState().selectFromDecision(nested ? 'previous' : fixture.task.id))
      if (nested) commit(() => useTasks.getState().select(fixture.task.id))
    }
    await act(async () => {})
    assert.ok(document.querySelector(pendingRead ? '[data-audit="tasksDetailId"]' : '[data-audit="taskCard"]'), 'real task host renders its pending or loaded detail')
  }
  const settleDestination = async (reject = false) => {
    assert.ok(pending, 'destination started')
    const held = pending
    pending = null
    await act(async () => reject ? held.reject(new Error('Destination unavailable')) : held.resolve('tab'))
  }
  const opened = async (selector, kind) => {
    await click(selector)
    assert.equal(fixture.navigations[0]?.[0], kind)
    assert.equal(visibleModals().length, 0, 'modal is gone while opening is still pending')
    assert.equal(document.activeElement, agent, 'focus belongs to the destination')
    await escape()
    assert.equal(visibleModals().length, 0, 'Escape after navigation cannot leave a task modal behind')
    await settleDestination()
    if (pending) await settleDestination()
  }
  for (const origin of [{}, { nested: true }, { waiting: true, nested: true }]) {
    await configure('card-with-live-run', origin)
    await opened('[data-audit="tasksOpenRun"]', 'run')
    await configure('card-spec-session', origin)
    await opened('[data-audit="specSessionOpen"]', 'session')
    await configure('card-spec-session', origin)
    await opened('[data-audit="specSessionResume"]', 'session')
    for (const [story, audit] of [['card-proposal-session', 'taskOpenProposalSession'], ['card-proposal-and-apply', 'taskOpenApplySession']]) {
      await configure(story, origin)
      await opened(`[data-audit="${audit}"]`, 'workflow')
    }
  }
  await configure('card-spec-session', { closedSession: true })
  assert.equal(document.querySelector('[data-audit="specSessionOpen"]').textContent.trim(), 'Reopen')
  await opened('[data-audit="specSessionOpen"]', 'session')
  for (const audit of ['taskSpecHeaderLink', 'taskOpenSpec', 'specOpenFile']) {
    await configure('card-spec-session', { nested: true })
    await opened(`[data-audit="${audit}"]`, audit === 'specOpenFile' ? 'file' : 'spec')
  }
  await configure()
  await opened('[data-audit="tasksFile"]', 'attachment')
  for (const [audit, kind] of [['tasksAttachmentOpen', 'attachment'], ['tasksAttachmentReveal', 'reveal']]) {
    await configure('card-with-live-run', { waiting: true, nested: true })
    await opened(`[data-audit="${audit}"]`, kind)
  }
  await configure('card-with-live-run', { image: true, waiting: true, nested: true })
  await click('[data-audit="tasksThumb"]')
  assert.equal(visibleModals().length, 2, 'image preview remains a nested dialog')
  await opened('[data-audit="attachmentLightboxOpen"]', 'attachment')
  await configure('card-with-live-run', { milestone: true, nested: true })
  await opened('[data-audit="taskMilestone"]', 'milestone')
  await configure('card-proposal-session', { nested: true })
  await opened('[data-audit="specProposeForTask"]', 'session')
  await configure('card-spec-session')
  await click('[data-audit="specSessionElsewhere"]')
  await opened('[data-audit="taskDispatchTarget"][data-kind="session"]', 'session')
  await configure('card-spec-session')
  await click('[data-audit="specSessionElsewhere"]')
  await opened('[data-audit="taskDispatchTarget"][data-kind="fresh"]', 'new-session')
  await configure('card-spec-session')
  await click('[data-audit="specSessionElsewhere"]')
  await click('[data-audit="taskDispatchTarget"][data-kind="role"]')
  assert.equal(fixture.navigations.length, 0, 'assigning a role stays in the card')
  assert.ok(document.querySelector('[data-audit="taskCard"]'))
  // Escape must still reach a modal if a removed control or background terminal took focus.
  let terminalEscapes = 0
  const terminalKey = (event) => { if (event.key === 'Escape') terminalEscapes++ }
  agent.addEventListener('keydown', terminalKey)
  agent.focus()
  await escape()
  assert.equal(visibleModals().length, 0, 'Escape closes the task after focus escaped')
  assert.equal(terminalEscapes, 0, 'the covered terminal must not receive Escape')
  await configure('card-with-live-run', { pendingRead: true })
  agent.focus()
  await escape()
  assert.equal(visibleModals().length, 0, 'Escape also closes a loading task after focus escaped')
  await configure()
  await click('[data-audit="taskFieldEdit"][data-field="title"]')
  agent.focus()
  await escape()
  assert.ok(document.querySelector('[data-audit="taskCard"]'), 'first Escape cancels an edit')
  assert.equal(document.querySelector('[data-audit="taskEditor"]'), null)
  assert.ok(document.querySelector('[data-audit="taskCard"]').contains(document.activeElement), 'cancelled edit restores card focus')
  await escape()
  assert.equal(visibleModals().length, 0, 'second Escape closes the card')
  agent.focus()
  await escape()
  assert.equal(terminalEscapes, 1, 'closing removes the outside-Escape listener')
  agent.removeEventListener('keydown', terminalKey)
  await configure('card-with-live-run', { verify: true })
  await click('[data-audit="taskVerifyLog"]')
  assert.equal(visibleModals().length, 2, 'verify log remains a nested dialog')
  await escape()
  assert.equal(document.querySelector('[data-audit="checkLogModal"]'), null, 'Escape closes only the innermost log')
  assert.ok(document.querySelector('[data-audit="taskCard"]'))
  await escape()
  assert.equal(visibleModals().length, 0)
  // Approval waits for a successful launch, then exits the entire trail before opening.
  await configure('card-spec-ready', { waiting: true, nested: true })
  let releaseLaunch
  fixture.launch = () => new Promise((resolve) => { releaseLaunch = resolve })
  let approving
  await act(async () => { approving = approveTaskSpec('p', fixture.task.id, fixture.task.change) })
  assert.ok(document.querySelector('[data-audit="taskCard"]'), 'pending approval keeps the card')
  await act(async () => releaseLaunch())
  assert.equal(fixture.navigations[0]?.[0], 'approve')
  assert.equal(visibleModals().length, 0)
  await settleDestination()
  await approving
  await configure('card-with-live-run', { nested: true })
  await click('[data-audit="tasksOpenRun"]')
  await settleDestination(true)
  assert.ok(fixture.failures[0].includes('Destination unavailable'), 'a failed navigation is visible after leaving the task')
  // An old asynchronous completion must not close a newly opened task.
  await configure('card-proposal-session')
  await click('[data-audit="taskOpenProposalSession"]')
  commit(() => useTasks.getState().select('previous'))
  await act(async () => {})
  await settleDestination()
  assert.equal(useTasks.getState().selected, 'previous', 'late workflow completion does not close a newer card')
  assert.ok(document.querySelector('[data-audit="taskCard"]'))
  await configure()
  await click('[data-audit="taskFieldEdit"][data-field="title"]')
  const title = document.querySelector('[data-audit="taskEditor"]')
  Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, 'value').set.call(title, 'Edited before navigation')
  await act(async () => title.dispatchEvent(new dom.window.Event('input', { bubbles: true })))
  await opened('[data-audit="tasksOpenRun"]', 'run')
  assert.ok(fixture.edits.some(([, , edit]) => edit.kind === 'setTitle' && edit.title === 'Edited before navigation'), 'outbound navigation commits a dirty field before leaving')
  // Back and local edits remain scoped to the card rather than becoming an outbound exit.
  await configure('card-spec-session', { waiting: true, nested: true })
  await click('[data-audit="taskBack"]')
  assert.ok(document.querySelector('[data-audit="taskCard"]'), 'Back keeps the preceding task open')
  await escape()
  assert.ok(document.querySelector('[data-audit="waitingDecisionDialog"]:not([inert])'), 'Escape back to Waiting preserves the decision')
  await configure('card-spec-session')
  const epoch = useTasks.getState().decisionEpoch
  commit(() => leaveTaskDetails('other-project', fixture.task.id))
  assert.equal(useTasks.getState().decisionEpoch, epoch, 'a stale launch cannot close another project')
  commit(() => leaveTaskDetails('p', 'other-task'))
  assert.equal(useTasks.getState().selected, fixture.task.id, 'a stale launch cannot close another task')
  // The Waiting origin is owned by AgentsPanelHost: pin the remount seam as well as driving it
  // through the real task store above, so forgetting that host wire cannot silently regress it.
  const host = readFileSync(join(ui, 'src/sidebar/AgentsPanel/AgentsPanelHost.tsx'), 'utf8')
    .replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '')
  assert.match(host, /const decisionEpoch = useTasks\(\(s\) => s\.decisionEpoch\)/)
  assert.match(host, /<WaitingPanel\s+key=\{`\$\{project\}:\$\{decisionEpoch\}`\}/)
  console.log('check-task-navigation: ok (real task host, all outbound controls, pending/failing destinations, nested Back/Waiting trails, focus and Escape)')
} finally {
  if (root) await act(async () => root.unmount())
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
  delete globalThis.taskNavigationCheck
}
