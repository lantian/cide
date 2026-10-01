/** Mount the real answer UI; verify payloads, visual choices and retry behavior. */
import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, rmSync } from 'node:fs'
import { resolve } from 'node:path'
import { JSDOM } from 'jsdom'
import React, { act } from 'react'

mkdirSync('node_modules/.cache', { recursive: true })
const out = mkdtempSync('node_modules/.cache/cide-task-questions-')
const dom = new JSDOM('<!doctype html><div id="root"></div>', { url: 'https://cide.test', pretendToBeVisual: true })
for (const key of ['window', 'document', 'HTMLElement', 'Element', 'Node', 'MutationObserver']) globalThis[key] = dom.window[key]
globalThis.IS_REACT_ACT_ENVIRONMENT = true
const { createRoot } = await import('react-dom/client')
let root
try {
  execFileSync('node', ['node_modules/vite/bin/vite.js', 'build', '--ssr',
    'src/sidebar/TasksPanel/questionSmoke.ts', '--outDir', out, '--logLevel', 'error'], { stdio: 'inherit' })
  const { QuestionAnswer, QuestionField, WaitingPanel, waitingFor, waitingCount } = await import(`file://${resolve(out, 'questionSmoke.js')}`)
  root = createRoot(document.querySelector('#root'))
  const mount = async (component, props, key = 'test') => act(async () => root.render(React.createElement(component, { key, ...props })))
  const click = async (element) => { assert.ok(element); await act(async () => element.click()) }
  const button = (text) => [...document.querySelectorAll('button')].find((b) => b.textContent === text)
  const type = async (text) => act(async () => {
    const textarea = document.querySelector('textarea')
    Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, 'value').set.call(textarea, text)
    textarea.dispatchEvent(new dom.window.Event('input', { bubbles: true }))
  })
  const base = { id: 't-1', title: 'Renders', status: 'review', acceptance: 'user', question: 'Pick a color' }
  const groups = waitingFor([base])
  assert.equal(groups.review.length, 0)
  assert.equal(groups.questions.length, 1)
  assert.equal(waitingCount([base]), 1)
  await mount(WaitingPanel, { project: 'p', title: 'Waiting', ...groups, reports: {}, busy: new Set(),
    onOpenTask() {}, onAccept() {}, onSendBack() {}, onAnswer() {} })
  assert.equal(button('Accept'), undefined, 'only question actions are offered')
  assert.ok(button('Answer'))

  const waitingProps = { project: 'p', title: 'Waiting', questions: [base],
    review: [{ ...base, id: 't-2', question: null }], reports: {}, busy: new Set(),
    onOpenTask() {}, onAccept() {}, onSendBack() {}, onAnswer() {} }
  await mount(WaitingPanel, waitingProps, 'groups')
  const [acceptGroup, questionGroup] = document.querySelectorAll('details')
  assert.ok(acceptGroup.open && questionGroup.open, 'waiting groups start expanded')
  const acceptTitle = acceptGroup.querySelector('summary')
  const questionTitle = questionGroup.querySelector('summary')
  await type('Keep this answer')
  await click(questionTitle)
  assert.equal(questionGroup.open, false, 'clicking the question group title collapses it')
  assert.equal(acceptGroup.open, true, 'groups toggle independently')
  assert.ok(questionTitle.textContent.includes('Questions1'), 'the collapsed group keeps its title and count')
  await mount(WaitingPanel, { ...waitingProps, reports: { 't-2': 'Updated report' } }, 'groups')
  assert.equal(questionGroup.open, false, 'board refreshes preserve collapsed groups')
  await click(questionTitle)
  assert.equal(questionGroup.open, true, 'clicking the title again expands it')
  assert.equal(document.querySelector('textarea').value, 'Keep this answer', 'collapsing preserves answer drafts')
  await click(button('Send back'))
  await act(async () => {
    const input = document.querySelector('[aria-label="Why t-2 goes back"]')
    Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, 'value').set.call(input, 'Keep this reason')
    input.dispatchEvent(new dom.window.Event('input', { bubbles: true }))
  })
  await click(acceptTitle)
  assert.equal(acceptGroup.open, false, 'clicking the acceptance group title collapses it')
  assert.equal(questionGroup.open, true)
  await click(acceptTitle)
  assert.equal(acceptGroup.open, true)
  assert.equal(document.querySelector('[aria-label="Why t-2 goes back"]').value, 'Keep this reason', 'collapsing preserves send-back drafts')

  const question = { text: 'Which renders do you prefer?', selection: 'multiple', options: Array.from({ length: 10 }, (_, i) => ({
    id: `render-${i}`, title: `Render ${i}`, description: `Description ${i}`, image: `image-${i}`,
  })) }
  const previews = Object.fromEntries(question.options.map((o) => [o.image, { kind: 'ready', url: `https://cide.test/${o.image}.png` }]))
  const calls = []
  let fail = true
  const onAnswer = async (task, payload) => { calls.push({ task, payload }); if (fail) throw new Error('Try again') }
  await mount(QuestionAnswer, { task: 't-1', question, previews, compact: true, onAnswer }, 'choices')
  assert.equal(document.querySelectorAll('input[type="checkbox"]').length, 10)
  assert.equal(document.querySelectorAll('img').length, 10)
  assert.ok(document.body.textContent.includes('Description 9'))
  assert.ok(button('Answer').disabled, 'nothing is selected automatically')
  await click(document.querySelector('input[value="render-0"]'))
  await click(document.querySelector('input[value="render-9"]'))
  await type('Darker background')
  await click(button('Compare choices'))
  assert.equal(document.querySelectorAll('input:checked').length, 2)
  assert.equal(document.querySelector('textarea').value, 'Darker background')
  await click(document.querySelector('[aria-label="View Render 0 full size"]'))
  assert.equal(document.querySelectorAll('img').length, 11, 'the full-size viewer opens')
  await click(button('Close'))
  assert.equal(document.querySelectorAll('input:checked').length, 2, 'previewing preserves selections')
  await click(button('Answer'))
  assert.equal(document.querySelector('[role="alert"]').textContent, 'Try again')
  assert.equal(document.querySelector('textarea').value, 'Darker background', 'a failed submission keeps the draft')
  assert.equal(document.querySelectorAll('input:checked').length, 2)
  fail = false
  await click(button('Answer'))
  assert.deepEqual(calls[1], { task: 't-1', payload: { kind: 'answer', text: 'Darker background', selectedIds: ['render-0', 'render-9'], expectedQuestion: question } })
  assert.equal(document.querySelectorAll('input:checked').length, 0)

  const single = { ...question, selection: 'single' }
  await mount(QuestionAnswer, { task: 't-1', question: single, previews, onAnswer }, 'single')
  await click(document.querySelector('input[value="render-0"]'))
  await click(document.querySelector('input[value="render-9"]'))
  assert.equal(document.querySelectorAll('input:checked').length, 1)
  await click(button('Answer'))
  assert.deepEqual(calls.at(-1).payload.selectedIds, ['render-9'])
  assert.equal(calls.at(-1).payload.text, '', 'selections alone are valid')
  await click(document.querySelector('input[value="render-0"]'))
  await click(button('Clear selection'))
  await type('None of these; try another material')
  await click(button('Answer'))
  assert.deepEqual(calls.at(-1).payload.selectedIds, [], 'custom-only is available after choosing a preset')
  assert.equal(calls.at(-1).payload.text, 'None of these; try another material')

  await mount(QuestionAnswer, { task: 't-1', question: 'Legacy question', onAnswer }, 'legacy')
  await type('Custom answer')
  await act(async () => document.querySelector('textarea').dispatchEvent(new dom.window.KeyboardEvent('keydown', { key: 'Enter', ctrlKey: true, bubbles: true })))
  assert.equal(calls.at(-1).payload.text, 'Custom answer')
  assert.deepEqual(calls.at(-1).payload.selectedIds, [])

  const editCalls = []
  await mount(QuestionField, { task: { ...base, question }, onSetQuestion: (...args) => editCalls.push(args), onAnswer }, 'field')
  await click(button('Edit'))
  await act(async () => {
    const input = document.querySelector('[aria-label="The question on t-1"]')
    Object.getOwnPropertyDescriptor(dom.window.HTMLInputElement.prototype, 'value').set.call(input, 'Updated question')
    input.dispatchEvent(new dom.window.Event('input', { bubbles: true }))
  })
  await click(button('Save'))
  assert.deepEqual(editCalls[0][1], { ...question, text: 'Updated question' }, 'editing the question preserves its choices')

  let release
  const pending = new Promise((resolve) => { release = resolve })
  let submissions = 0
  await mount(QuestionAnswer, { task: 't-1', question: single, previews, onAnswer: () => { submissions++; return pending } }, 'pending')
  await click(document.querySelector('input[value="render-0"]'))
  await act(async () => { button('Answer').click(); button('Answer').click() })
  assert.equal(submissions, 1)
  assert.ok(button('Answer').disabled)
  await act(async () => release())

  const large = { ...question, options: Array.from({ length: 120 }, (_, i) => ({ id: `${i}`, title: `Choice ${i}` })) }
  await mount(QuestionAnswer, { task: 't-1', question: large, onAnswer }, 'large')
  assert.equal(document.querySelectorAll('input[type="checkbox"]').length, 120, 'options are not truncated')
  await mount(QuestionAnswer, { task: 't-1', question, previews: { 'image-0': { kind: 'refused', reason: 'Image missing' } }, onAnswer }, 'missing')
  assert.ok(document.body.textContent.includes('Image missing'))
  assert.equal(document.querySelectorAll('input[type="checkbox"]').length, 10, 'missing images keep their answer choices')
  console.log('check-task-questions: ok (collapsible groups, exclusive actions, images, single/multiple/custom answers, retries, editing and duplicate submission)')
} finally {
  if (root) await act(async () => root.unmount())
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
