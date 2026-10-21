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
  const { QuestionAnswer, QuestionField, WaitingPanel, AgentsPanelTabs, waitingDecisionCount, TaskDetailPendingModal, waitingFor, waitingCount } = await import(`file://${resolve(out, 'questionSmoke.js')}`)
  root = createRoot(document.querySelector('#root'))
  const mount = async (component, props, key = 'test') => act(async () => root.render(React.createElement(component, { key, ...props })))
  const click = async (element) => { assert.ok(element); await act(async () => element.click()) }
  const visible = (element) => !element.closest('[inert]')
  const button = (text) => [...document.querySelectorAll('button')].find((b) => visible(b) && b.textContent === text)
  const type = async (text) => act(async () => {
    const textarea = [...document.querySelectorAll('textarea')].find(visible)
    Object.getOwnPropertyDescriptor(dom.window.HTMLTextAreaElement.prototype, 'value').set.call(textarea, text)
    textarea.dispatchEvent(new dom.window.Event('input', { bubbles: true }))
  })
  const base = { id: 't-1', title: 'Renders', status: 'review', acceptance: 'user', question: 'Pick a color' }
  const groups = waitingFor([base])
  assert.equal(groups.review.length, 0)
  assert.equal(groups.questions.length, 1)
  assert.equal(waitingCount([base]), 1)
  const decisionCalls = []
  const waitingProps = { project: 'p', title: 'Waiting', questions: [base],
    review: [{ ...base, id: 't-2', title: 'Review result', question: null }],
    reports: { 't-2': 'Full report with the implementation details.' }, busy: new Set(),
    onOpenTask(id) { decisionCalls.push(['open', id]) },
    onAccept(id) { decisionCalls.push(['accept', id]) },
    onSendBack(id, note) { decisionCalls.push(['back', id, note]) },
    onAnswer(id, answer) { decisionCalls.push(['answer', id, answer]) } }
  await mount(WaitingPanel, waitingProps, 'decisions')
  assert.equal(document.querySelector('[role="dialog"]'), null, 'rows do not open a task or show inline actions')
  assert.equal(button('Answer'), undefined)
  assert.equal(button('Accept'), undefined)
  await click(button('Pick a color'))
  assert.ok(button('Answer').disabled)
  await type('Keep this answer')
  await click([...document.querySelectorAll('[aria-label="Close"]')].find(visible))
  await click(button('Pick a color'))
  assert.equal(document.querySelector('textarea').value, 'Keep this answer', 'closing the decision keeps its draft')
  await click(button('Open task t-1'))
  assert.deepEqual(decisionCalls.at(-1), ['open', 't-1'])
  await mount(WaitingPanel, { ...waitingProps, taskOpen: true }, 'decisions')
  assert.equal(document.querySelector('[data-audit="waitingDecisionDialog"]').closest('[inert]').style.display, 'none')
  await mount(WaitingPanel, waitingProps, 'decisions')
  assert.equal(document.querySelector('textarea').value, 'Keep this answer', 'returning from a task keeps the answer')
  await click(button('Answer'))
  assert.equal(decisionCalls.at(-1)[2].text, 'Keep this answer')
  await click(button('Review result'))
  assert.ok(document.body.textContent.includes('Full report with the implementation details.'))
  await click(button('Send back'))
  await type('Keep this reason')
  await click([...document.querySelectorAll('[aria-label="Close"]')].find(visible))
  await click(button('Review result'))
  assert.equal([...document.querySelectorAll('textarea')].at(-1).value, 'Keep this reason')
  await click([...document.querySelectorAll('button')].find((b) => b.textContent === 'Send back' && b.dataset.audit === 'waitingSendBackSubmit'))
  assert.deepEqual(decisionCalls.at(-1), ['back', 't-2', 'Keep this reason'])

  const origin = { id: 'conversation', known: true, kind: 'planner', harness: 'codex', title: 'Plan', cwd: '/project', startedUnixMs: 1, lastSeenUnixMs: 1 }
  const referenceQuestion = { text: 'Вопрос: t-1388, (t-1231). Не xt-5 и t-7extra. <b>Текст</b>', selection: 'single', options: [{ id: 'wait_production', title: 'Обе иконки после production GLB' }] }
  const referenceRequest = { id: 'reference-1', title: 'Choose model sources', message: referenceQuestion.text, task: 't-1', origin,
    author: { kind: 'orchestrator' }, wait: false, createdUnixMs: 1, response: null, applying: false, delivered: false, deliveryError: null,
    request: { kind: 'question', question: referenceQuestion } }
  const referenceProps = { ...waitingProps, questions: [], review: [], requests: [referenceRequest], onOpenSession() {}, onRespondRequest: async () => {} }
  const savedRequest = { ...referenceRequest, response: { kind: 'answer', text: 'Production only', selectedIds: ['wait_production'] } }
  const countBadge = () => document.querySelector('[data-audit="waitingCount"]')
  const answeredBadge = () => document.querySelector('[data-audit="waitingAnswered"]')
  const headerCount = () => document.querySelector('[data-audit="waitingHeaderCount"]').textContent
  function WaitingStatus(props) {
    return React.createElement(WaitingPanel, { ...props, title: React.createElement(AgentsPanelTabs, {
      tab: 'waiting', onTab() {}, waiting: waitingDecisionCount(props.requests, [...props.questions, ...props.review]),
    }) })
  }
  let failResponse = true
  function AnswerTransition() {
    const [requests, setRequests] = React.useState([referenceRequest])
    return React.createElement(WaitingStatus, { ...referenceProps, questions: [base], requests,
      onRespondRequest: async (id, response) => {
        if (failResponse) throw new Error('Answer was not saved')
        setRequests((now) => now.map((request) => request.id === id ? { ...request, response } : request))
      },
    })
  }
  await mount(AnswerTransition, {}, 'answer-transition')
  assert.equal(countBadge().textContent, '1', 'the linked task and its request count once')
  assert.equal(headerCount(), '1')
  assert.equal(answeredBadge(), null)
  await click(button('Choose model sources'))
  await click(document.querySelector('input[value="wait_production"]'))
  await click(button('Answer'))
  assert.equal(document.querySelector('[role="alert"]').textContent, 'Answer was not saved')
  assert.equal(countBadge().textContent, '1', 'a failed save still needs an answer')
  assert.equal(answeredBadge(), null)
  failResponse = false
  await click(button('Answer'))
  assert.equal(answeredBadge().textContent, 'Answered')
  assert.equal(answeredBadge().querySelector('[data-tone]').dataset.tone, 'blue')
  assert.ok(answeredBadge().title.includes('waiting for the agent'))
  assert.equal(countBadge(), null, 'the last saved answer hides the Waiting badge before delivery')
  assert.equal(headerCount(), '', 'the panel header uses the same count')
  assert.equal(document.querySelector('[data-audit="waitingEmpty"]'), null, 'answered rows remain available')
  await click(button('Choose model sources'))
  assert.ok(document.querySelector('[data-audit="waitingSavedDecision"]'))
  assert.equal(button('Answer'), undefined, 'a saved answer cannot be submitted again')
  await mount(WaitingStatus, { ...referenceProps, requests: [savedRequest], questions: [base] }, 'restored-answer')
  assert.equal(countBadge(), null, 'a freshly mounted panel restores the saved-answer state')
  assert.equal(answeredBadge().textContent, 'Answered')
  await mount(WaitingStatus, { ...referenceProps, requests: [{ ...savedRequest, deliveryError: 'Agent busy', applying: true }], questions: [base] }, 'restored-answer')
  assert.equal(countBadge(), null, 'application and delivery delays do not require another answer')
  assert.equal(answeredBadge().textContent, 'Answered')
  await mount(WaitingStatus, { ...referenceProps, requests: [{ ...referenceRequest, deliveryError: 'Try again' }], questions: [base] }, 'restored-answer')
  assert.equal(countBadge().textContent, '1', 'resetting a saved decision restores the count')
  assert.equal(answeredBadge(), null)
  const mixedProps = { ...waitingProps, requests: [savedRequest, { ...referenceRequest, id: 'open', title: 'Another question', task: null }],
    questions: [base, { ...base, id: 't-3', question: 'Standalone question' }] }
  await mount(WaitingStatus, mixedProps, 'mixed-waiting')
  assert.equal(countBadge().textContent, '3', 'only the open request, standalone question and review need the user')
  assert.equal(headerCount(), '3')
  assert.deepEqual([...document.querySelectorAll('[data-audit="waitingOpen"]')].map((row) => row.textContent),
    ['Choose model sources', 'Another question', 'Standalone question', 'Review result'], 'saved answers stay in their original position')
  const proposedMove = { kind: 'moveTasks', milestone: 'visuals', goal: 't-10', tasks: [{ id: 't-11', title: 'First task', status: 'inbox', parent: null }] }
  for (const response of [{ kind: 'approve', selectedIds: ['t-11'], note: '' }, { kind: 'reject', note: '' }, { kind: 'cancel' }]) {
    await mount(WaitingStatus, { ...referenceProps, requests: [{ ...referenceRequest, task: null, request: proposedMove, response }] }, 'other-decisions')
    assert.equal(answeredBadge().textContent, 'Decision saved')
    assert.equal(countBadge(), null)
  }
  await mount(WaitingStatus, { ...referenceProps, requests: [{ ...savedRequest, delivered: true }] }, 'other-decisions')
  assert.equal(answeredBadge(), null)
  assert.equal(document.querySelector('[data-audit="waitingOpen"]'), null, 'delivered answers leave the list')
  assert.equal(countBadge(), null)
  function ReferenceNavigation() {
    const [task, setTask] = React.useState(null)
    return React.createElement(React.Fragment, null,
      React.createElement(WaitingPanel, { ...referenceProps, taskOpen: task !== null, onOpenTask: setTask }),
      task === null ? null : React.createElement(TaskDetailPendingModal, { task: { ...base, id: task }, onClose: () => setTask(null), onBack: () => setTask(null) }))
  }
  await mount(ReferenceNavigation, {}, 'references')
  await click(button('Choose model sources'))
  const referenceLinks = [...document.querySelectorAll('[data-audit="questionTaskReference"]')]
  assert.deepEqual(referenceLinks.map((b) => b.textContent), ['t-1388', 't-1231'], 'task IDs link without matching pieces of other words')
  assert.equal(document.querySelector('[data-audit="waitingQuestion"] b'), null, 'question text is escaped')
  await click(document.querySelector('input[value="wait_production"]'))
  await type('Keep this draft while checking the task')
  await click(referenceLinks[0])
  assert.equal(document.querySelector('[data-audit="tasksDetailId"]').textContent.trim(), 't-1388')
  assert.ok(button('Back').closest('[data-audit="taskCardHead"]'), 'Back belongs in the task header, including while loading')
  await click(button('Back'))
  assert.equal([...document.querySelectorAll('textarea')].find(visible).value, 'Keep this draft while checking the task')
  assert.ok(document.querySelector('input[value="wait_production"]').checked)
  assert.equal(document.activeElement, referenceLinks[0], 'returning focuses the reference that opened the task')
  await mount(WaitingPanel, { ...referenceProps, requests: [{ ...referenceRequest, response: { kind: 'answer', text: 'Production only', selectedIds: ['wait_production'] } }] }, 'saved-answer')
  await click(button('Choose model sources'))
  assert.ok(document.querySelector('[role="status"]').textContent.includes('current turn ends'), 'nonblocking delivery explains why the saved answer waits')
  const savedAnswer = document.querySelector('[data-audit="waitingSavedDecision"]')
  assert.ok(savedAnswer.textContent.includes('Обе иконки после production GLB'))
  assert.ok(savedAnswer.textContent.includes('Production only'))
  assert.ok(!savedAnswer.textContent.includes('selectedIds') && !savedAnswer.textContent.includes('wait_production'), 'saved answers show choice labels instead of transport JSON')
  await mount(WaitingPanel, { ...referenceProps, requests: [{ ...referenceRequest, delivered: true }] }, 'saved-answer')
  assert.equal(document.querySelector('[data-audit="waitingDecisionDialog"]'), null, 'delivery removes the answered decision')

  const moveRequest = { id: 'move-1', title: 'Approve the plan', message: 'Include the useful work?', task: null,
    origin, author: { kind: 'orchestrator' }, wait: true, createdUnixMs: 1, response: null,
    applying: false, delivered: false, deliveryError: null,
    request: { kind: 'moveTasks', milestone: 'visuals', goal: 't-10', tasks: [
      { id: 't-11', title: 'First task', status: 'inbox', parent: null },
      { id: 't-12', title: 'Second task', status: 'todo', parent: 't-9' },
    ] } }
  const requestCalls = []
  const moveProps = { ...waitingProps, questions: [], review: [], requests: [moveRequest],
    onRespondRequest: async (id, response) => requestCalls.push([id, response]) }
  await mount(WaitingPanel, moveProps, 'moves')
  await click(button('Approve the plan'))
  assert.equal(document.querySelectorAll('input:checked').length, 2, 'all proposed tasks start included')
  await click([...document.querySelectorAll('label')].find((l) => l.textContent === 'Include t-12')?.querySelector('input'))
  await type('Only the first one')
  await click(button('View task'))
  await mount(WaitingPanel, { ...moveProps, taskOpen: true }, 'moves')
  await mount(WaitingPanel, moveProps, 'moves')
  assert.equal(document.querySelectorAll('input:checked').length, 1)
  assert.equal(document.querySelector('textarea').value, 'Only the first one')
  await click(button('Accept selected (1)'))
  assert.deepEqual(requestCalls.at(-1), ['move-1', { kind: 'approve', selectedIds: ['t-11'], note: 'Only the first one' }])
  await click(button('Approve the plan'))
  await click([...document.querySelectorAll('[aria-label="Close"]')].find(visible))
  assert.equal(requestCalls.length, 1, 'closing is not cancellation')
  await click(button('Approve the plan'))
  await click(button('Cancel request'))
  assert.deepEqual(requestCalls.at(-1), ['move-1', { kind: 'cancel' }])

  const question = { text: 'Which renders do you prefer?', selection: 'multiple', options: Array.from({ length: 10 }, (_, i) => ({
    id: `render-${i}`, title: `Render ${i}`, description: `Description ${i}`, image: `image-${i}`,
  })) }
  const previews = Object.fromEntries(question.options.map((o) => [o.image, { kind: 'ready', url: `https://cide.test/${o.image}.png` }]))
  const viewer = () => document.querySelector('[data-audit="imageViewer"]')
  const viewKey = async (key) => act(async () => document.activeElement.dispatchEvent(new dom.window.KeyboardEvent('keydown', { key, bubbles: true, cancelable: true })))
  const choiceRequest = { ...referenceRequest, title: 'Compare image choices', request: { kind: 'question', question } }
  await mount(WaitingPanel, { ...referenceProps, requests: [choiceRequest], previews }, 'image-decision')
  await click(button('Compare image choices'))
  await click(document.querySelector('input[value="render-2"]'))
  await type('Keep the layout and this draft')
  const imageOpener = document.querySelector('[aria-label="View Render 4 full size"]')
  imageOpener.focus()
  await click(imageOpener)
  assert.equal(viewer().getAttribute('aria-label'), 'Render 4')
  assert.ok(viewer().textContent.includes('5 / 10'), 'the decision supplies the whole gallery')
  assert.ok(document.querySelector('[data-audit="waitingDecisionDialog"]').closest('[inert]'), 'the decision is suspended below the image')
  await viewKey('ArrowRight')
  assert.equal(viewer().getAttribute('aria-label'), 'Render 5')
  await click(viewer().querySelector('[aria-label="Previous image"]'))
  assert.equal(viewer().getAttribute('aria-label'), 'Render 4')
  await viewKey('Escape')
  assert.equal(viewer(), null)
  assert.ok(document.querySelector('[data-audit="waitingDecisionDialog"]'), 'Escape closes only the image')
  assert.equal([...document.querySelectorAll('textarea')].find(visible).value, 'Keep the layout and this draft')
  assert.ok(document.querySelector('input[value="render-2"]').checked)
  assert.equal(document.activeElement, imageOpener)

  const reportAttachments = [
    { id: 'report-a', name: 'First report image', kind: 'image' },
    { id: 'report-file', name: 'Notes.txt', kind: 'file' },
    { id: 'report-b', name: 'Second report image', kind: 'image' },
  ]
  await mount(WaitingPanel, { ...waitingProps, questions: [], reportAttachments: { 't-2': reportAttachments },
    previews: { 'report-a': previews['image-0'], 'report-b': previews['image-1'] } }, 'report-images')
  await click(button('Review result'))
  await click(button('Send back'))
  await type('Keep report feedback')
  await click(document.querySelector('[aria-label="View Second report image full size"]'))
  assert.equal(viewer().getAttribute('aria-label'), 'Second report image')
  assert.ok(viewer().textContent.includes('2 / 2'), 'the report gallery excludes ordinary files')
  await viewKey('ArrowLeft')
  assert.equal(viewer().getAttribute('aria-label'), 'First report image')
  await click(button('Back'))
  assert.equal([...document.querySelectorAll('textarea')].find(visible).value, 'Keep report feedback')
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
  await viewKey('ArrowRight')
  assert.equal(viewer().getAttribute('aria-label'), 'Render 1', 'standalone comparison uses the same gallery')
  await viewKey('ArrowLeft')
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
  const nativeCalls = []
  let rejectNative = true
  let releaseNative
  const nativePending = new Promise((resolve) => { releaseNative = resolve })
  const batchRequest = { ...referenceRequest, id: 'native-batch', task: null, title: 'Planning choices', request: { kind: 'questions',
    source: { session: 'worker', generation: 'assignment', conversation: 'native-thread', request: 'call-1' },
    questions: [
      { id: 'language', header: 'Language', custom: false, question: { text: 'Which language?', selection: 'single', options: [{ id: '0', title: 'Русский', description: 'Native labels survive the round trip' }] } },
      { id: 'features', header: 'Features', custom: true, question: { text: 'Which features?', selection: 'multiple', options: [{ id: '0', title: 'Search' }, { id: '1', title: 'Export' }] } },
    ] } }
  await mount(WaitingPanel, { ...referenceProps, requests: [batchRequest], onRespondRequest: async (id, response) => {
    nativeCalls.push({ id, response })
    if (rejectNative) throw new Error('Answer could not be saved')
    await nativePending
  } }, 'native-batch')
  await click(button('Planning choices'))
  assert.equal(document.querySelectorAll('[data-audit="nativeQuestionsForm"]').length, 1)
  assert.ok(button('Answer').disabled, 'a batch never submits recommended defaults automatically')
  assert.equal(document.querySelectorAll('textarea').length, 1, 'custom input follows each native question capability')
  await click(document.querySelector('input[type="radio"]'))
  assert.ok(button('Answer').disabled, 'every question in the native call must have an answer')
  await click(document.querySelector('input[type="checkbox"][value="0"]'))
  await click(document.querySelector('input[type="checkbox"][value="1"]'))
  await type('Custom details\nSecond line')
  await click(button('Answer'))
  assert.ok(document.body.textContent.includes('Answer could not be saved'))
  assert.equal(document.querySelectorAll('input:checked').length, 3)
  assert.equal(document.querySelector('textarea').value, 'Custom details\nSecond line')
  await click([...document.querySelectorAll('[aria-label="Close"]')].find(visible))
  await click(button('Planning choices'))
  assert.equal(document.querySelector('textarea').value, 'Custom details\nSecond line', 'dismissal preserves the entire batch draft')
  rejectNative = false
  await act(async () => { button('Answer').click(); button('Answer').click() })
  assert.equal(nativeCalls.length, 2, 'a pending batch is submitted once')
  assert.deepEqual(nativeCalls.at(-1), { id: 'native-batch', response: { kind: 'answers', answers: [
    { id: 'language', text: '', selectedIds: ['0'] }, { id: 'features', text: 'Custom details\nSecond line', selectedIds: ['0', '1'] },
  ] } })
  await act(async () => releaseNative())
  console.log('check-task-questions: ok (native grouped questions, complete batches, preserved drafts, capability-aware custom input, retries and duplicate submission; legacy decisions, images and task questions)')
} finally {
  if (root) await act(async () => root.unmount())
  dom.window.close()
  rmSync(out, { recursive: true, force: true })
}
