/** Exercise the real card and feedback links without starting an agent or changing a tracker. */
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { resolve } from 'node:path'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync('node_modules/.cache/cide-task-dispatch-')
const dom = new JSDOM('<!doctype html><div id="root"></div>', { url: 'https://cide.test', pretendToBeVisual: true })
for (const key of ['window', 'document', 'HTMLElement', 'Element', 'Node', 'MutationObserver']) globalThis[key] = dom.window[key]
globalThis.IS_REACT_ACT_ENVIRONMENT = true
const { createRoot } = await import('react-dom/client')
let root
try {
  execFileSync('node', ['node_modules/vite/bin/vite.js', 'build', '--ssr', 'src/sidebar/TasksPanel/dispatchSmoke.ts', '--outDir', out, '--logLevel', 'error'], { stdio: 'inherit' })
  const { TaskDispatchInfo, TaskDetail, CARD_STORIES, dispatchNotice, useTasks } = await import(`file://${resolve(out, 'dispatchSmoke.js')}`)
  root = createRoot(document.querySelector('#root'))
  const calls = []
  const actions = { onOpenTask: (id) => calls.push(['task', id]), onOpenMilestone: (id) => calls.push(['milestone', id]), onOpenRun: (id) => calls.push(['run', id]) }
  const base = { project: 'project-a', task: 't-1488', agent: 'developer', label: 'Developer', state: 'blocked', run: null,
    reasons: [
      { code: 'dependency', text: 'Blocked by t-1489 — Todo. The dependency must be Done.', task: 't-1489', milestone: null },
      { code: 'inactiveMilestone', text: 'Milestone playtest-oct is inactive. Current milestone: playtest.', task: null, milestone: 'playtest-oct' },
    ] }
  const mount = async (status, extra = {}) => act(async () => root.render(React.createElement(TaskDispatchInfo, { status, ...actions, ...extra })))
  const button = (label) => [...document.querySelectorAll('button')].find((el) => el.textContent === label)
  const click = async (label) => { const el = button(label); assert.ok(el, label); await act(async () => el.click()) }
  for (const theme of ['light', 'dark']) for (const scale of ['0.846', '1.308']) {
    document.documentElement.dataset.theme = theme
    document.documentElement.style.setProperty('--ui-scale', scale)
    await mount(base)
    assert.equal(document.querySelectorAll('li').length, 2)
    assert.ok(document.body.textContent.includes('Agent not started · Developer'))
    assert.ok(document.querySelector('[role="status"]'))
    assert.ok(!document.body.innerHTML.includes('undefined'))
  }
  await click('Open t-1489')
  await click('Open playtest-oct')
  assert.deepEqual(calls, [['task', 't-1489'], ['milestone', 'playtest-oct']])
  await mount({ ...base, reasons: base.reasons.slice(1) })
  assert.equal(button('Open t-1489'), undefined)
  assert.equal(document.querySelectorAll('li').length, 1)
  await mount({ ...base, state: 'ready', reasons: [] })
  assert.ok(document.body.textContent.includes('Can be started'))
  assert.ok(document.body.textContent.includes('Assign the role again'))
  assert.equal(document.querySelectorAll('li').length, 0)
  for (const state of ['queued', 'paused', 'failed']) {
    await mount({ ...base, state, run: 'run-1', reasons: [{ code: 'wait', text: 'Waiting for a slot', task: null, milestone: null }] }, { openable: true })
    assert.ok(document.body.textContent.includes(state === 'failed' ? 'Agent startup failed' : `Agent ${state}`))
    if (state === 'queued') assert.equal(button('Open run'), undefined, 'a queued run has no session to open')
    else {
      await click('Open run')
      assert.deepEqual(calls.at(-1), ['run', 'run-1'])
    }
  }
  await mount({ ...base, state: 'failed', run: 'run-1' })
  assert.equal(button('Open run'), undefined, 'a failure before spawn has no session to open')
  for (const state of ['running', 'idle']) {
    await mount({ ...base, state, reasons: [] })
    assert.equal(document.querySelector('[data-audit="taskDispatchInfo"]'), null)
  }
  await mount(null, { error: 'Connection lost' })
  assert.ok(document.body.textContent.includes('Unable to check agent startup: Connection lost'))
  await mount({ ...base, reasons: [{ code: 'failure', text: '<img src=x onerror=alert(1)>', task: null, milestone: null }] })
  assert.equal(document.querySelector('img'), null)
  const notice = dispatchNotice(base)
  assert.equal(notice.project, 'project-a')
  assert.equal(notice.kind, 'warn')
  assert.ok(notice.text.includes('t-1488 · Developer'))
  assert.equal(notice.hint, 'Blocked by t-1489 · Milestone playtest-oct is inactive')
  assert.ok(notice.detail.includes('Current milestone: playtest'))
  assert.equal(dispatchNotice({ ...base, state: 'failed' }).kind, 'error')
  const mentioned = { ...base, agent: 'qa', label: 'QA' }
  assert.ok(dispatchNotice(mentioned).text.includes('QA'))
  useTasks.setState({ project: 'project-a', selected: 't-1488', dispatchAgent: 'qa' })
  useTasks.getState().select('t-1489')
  assert.equal(useTasks.getState().dispatchAgent, null, 'a mention target does not carry into another task')
  useTasks.setState({ dispatchAgent: 'qa' })
  useTasks.getState().selectFromDecision('t-1490')
  assert.equal(useTasks.getState().dispatchAgent, null, 'opening a decision also resets the mention target')
  useTasks.setState({ dispatchAgent: 'qa' })
  await useTasks.getState().attach(null)
  assert.equal(useTasks.getState().dispatchAgent, null, 'leaving a project clears its role context')
  await act(async () => root.render(React.createElement(TaskDetail, { ...CARD_STORIES.card, dispatchStatus: base, ...actions })))
  assert.ok(document.querySelector('[data-audit="taskDispatchInfo"]'), 'the real task card includes the explanation')
  const body = [...document.querySelectorAll('[data-field]')].find((el) => el.dataset.field === 'body')
  if (body) assert.ok(document.querySelector('[data-audit="taskDispatchInfo"]').compareDocumentPosition(body) & dom.window.Node.DOCUMENT_POSITION_FOLLOWING)
  console.log('task-dispatch: PASS — multiple reasons, resolution, navigation, queues, notices, safe text and real card rendering')
} finally {
  if (root) await act(async () => root.unmount())
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
