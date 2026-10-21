/** Every Waiting row opens a decision; task cards and image viewers are child views. */
import { useEffect, useId, useRef, useState, type ReactNode } from 'react'
import type { UserRequest, UserRequestResponse } from '@/ipc/generated'
import { Button } from '@/kit/components/Button'
import { Checkbox } from '@/kit/components/Choice'
import { Textarea } from '@/kit/components/Field'
import { Dialog } from '@/kit/components/Overlay'
import { Disclosure, List, ListItem } from '@/kit/components/Surface'
import { Badge, Code } from '@/kit/components/Status'
import { Modal } from '@/overlays/ModalShell'
import { ImageViewer, type ImageGallery } from '@/overlays/ImageViewer'
import { questionText, type TaskView, type AttachmentView, type AttachmentPreview } from '@/sidebar/TasksPanel/model'
import { QuestionAnswer, QuestionImage, questionImages, type AnswerHandler } from '@/sidebar/TasksPanel/QuestionAnswer'
import { TaskMarkdown } from '@/sidebar/TasksPanel/TaskMarkdown'
import panelStyles from '../TasksPanel/TasksPanel.module.css'
import styles from './WaitingPanel.module.css'
import { waitingDecisionCount } from './waitingModel'
import { GroupedQuestions } from './GroupedQuestions'

export interface WaitingPanelProps {
  project: string | null
  title: ReactNode
  review: readonly TaskView[]
  questions: readonly TaskView[]
  reports: Readonly<Record<string, string | null>>
  reportAttachments?: Readonly<Record<string, readonly AttachmentView[]>> | undefined
  busy: ReadonlySet<string>
  requests?: readonly UserRequest[] | undefined
  requestError?: string | null | undefined
  taskOpen?: boolean | undefined
  onOpenTask: (task: string) => void
  onOpenSession?: ((id: string) => void) | undefined
  onAccept: (task: string) => void | Promise<void>
  onSendBack: (task: string, note: string) => void | Promise<void>
  onAnswer: AnswerHandler
  onRespondRequest?: ((id: string, response: UserRequestResponse) => Promise<void>) | undefined
  previews?: Readonly<Record<string, AttachmentPreview>> | undefined
  onRequestPreview?: ((task: string, image: string) => void) | undefined
}
type Item = { id: string; title: string; label: string; task: TaskView | null; request: UserRequest | null }

export function WaitingPanel(props: WaitingPanelProps) {
  const [selected, setSelected] = useState<string | null>(null)
  const [opened, setOpened] = useState<ReadonlySet<string>>(new Set())
  const open = (id: string) => { setOpened((now) => new Set([...now, id])); setSelected(id) }
  useEffect(() => { setSelected(null); setOpened(new Set()) }, [props.project])
  const requests = props.requests ?? []
  const linked = new Set(requests.filter((r) => r.request.kind === 'question' && !r.delivered).map((r) => r.task))
  const items: Item[] = [
    ...requests.filter((r) => !r.delivered).map((request) => ({ id: request.id, title: request.title, label: request.request.kind === 'moveTasks' ? 'Move tasks' : 'Question', task: null, request })),
    ...props.questions.filter((t) => !linked.has(t.id)).map((task) => ({ id: `question:${task.id}`, title: task.question === null ? task.title : questionText(task.question), label: 'Question', task, request: null })),
    ...props.review.filter((t) => !linked.has(t.id)).map((task) => ({ id: `review:${task.id}`, title: task.title, label: 'Accept result', task, request: null })),
  ]
  const waiting = waitingDecisionCount(requests, [...props.questions, ...props.review])
  return (
    <aside className={panelStyles.panel} data-audit="sidebarWaiting" aria-label="Waiting for you">
      <div className={panelStyles.header} data-audit="agentsHeader"><span className={panelStyles.headerTitle}>{props.title}</span><span className={panelStyles.headerMeta} data-audit="waitingHeaderCount">{waiting || ''}</span></div>
      <div className={panelStyles.body} data-audit="waitingBody">
        {props.requestError && <p role="alert" className={styles.error}>{props.requestError}</p>}
        {props.project === null ? <p className={panelStyles.waitQuiet}>Open a project to see what waits for you.</p> : items.length === 0 ? <p className={panelStyles.waitQuiet} data-audit="waitingEmpty">Nothing is waiting for you.</p> : (
          <List label="Decisions waiting for you">
            {items.map((i) => <ListItem key={i.id} top={<Code>{i.label}</Code>}
              topAside={i.request?.response != null ? <span data-audit="waitingAnswered" title="Decision saved; waiting for the agent.">
                <Badge tone="blue" soft>{i.request.response.kind === 'answer' || i.request.response.kind === 'answers' ? 'Answered' : 'Decision saved'}</Badge>
              </span> : undefined}
              title={<Button variant="link" size="sm" data-audit="waitingOpen" onClick={() => open(i.id)}>{i.title}</Button>}
              meta={i.request ? `${i.request.origin.title} · ${new Date(Number(i.request.createdUnixMs)).toLocaleString()}` : i.task?.id}
              onOpen={() => open(i.id)} />)}
          </List>
        )}
      </div>
      {items.filter((item) => opened.has(item.id)).map((item) => <Decision key={`${props.project}:${item.id}`} {...props} open={selected === item.id} item={item} onClose={() => setSelected(null)} />)}
    </aside>
  )
}

function Decision({ item, onClose, open, ...props }: WaitingPanelProps & { item: Item; onClose: () => void; open: boolean }) {
  const formId = useId()
  const [validAnswer, setValidAnswer] = useState(false)
  const [note, setNote] = useState('')
  const [sendBack, setSendBack] = useState(false)
  const [sending, setSending] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [viewing, setViewing] = useState<ImageGallery | null>(null)
  const request = item.request
  const move = request?.request.kind === 'moveTasks' ? request.request : null
  const [checked, setChecked] = useState<string[]>(() => move?.tasks.map((t) => t.id) ?? [])
  const returnFocus = useRef<HTMLButtonElement | null>(null)
  const inFlight = useRef(false)
  const question = request?.request.kind === 'question' ? request.request.question : item.task?.question ?? null
  const questions = request?.request.kind === 'questions' ? request.request.questions : null
  const task = request?.task ?? item.task?.id ?? ''
  const images = question !== null ? questionImages(question) : (props.reportAttachments?.[task] ?? [])
    .filter((attachment) => attachment.kind === 'image')
    .map((attachment) => ({ id: attachment.id, title: attachment.name, previewId: attachment.id }))
  const busy = sending || props.busy.has(task) || request?.applying === true
  const resolved = request?.response != null
  useEffect(() => { if (!props.taskOpen) returnFocus.current?.focus() }, [props.taskOpen])
  const run = async (work: () => void | Promise<void>) => {
    if (busy || inFlight.current) return
    inFlight.current = true; setSending(true); setError(null)
    try { await work(); onClose() } catch (reason) { setError(reason instanceof Error ? reason.message : String(reason)) }
    finally { inFlight.current = false; setSending(false) }
  }
  const respond = (response: UserRequestResponse) => {
    if (!request || !props.onRespondRequest) return Promise.reject(new Error('This request cannot be answered right now.'))
    return props.onRespondRequest(request.id, response)
  }
  const openTask = (id: string, button: HTMLButtonElement) => { returnFocus.current = button; props.onOpenTask(id) }
  const actions = resolved ? <Button onClick={onClose}>Close</Button> : (
    <>
      {request && <Button variant="link" disabled={busy} onClick={() => void run(() => respond({ kind: 'cancel' }))}>Cancel request</Button>}
      {move ? <>
        <Button disabled={busy} onClick={() => void run(() => respond({ kind: 'reject', note }))}>Reject</Button>
        <Button variant="primary" busy={busy} disabled={busy || checked.length === 0} data-audit="waitingApproveMoves"
          onClick={() => void run(() => respond({ kind: 'approve', selectedIds: checked, note }))}>Accept selected ({checked.length})</Button>
      </> : question !== null || questions !== null ? <Button type="submit" form={formId} variant="primary" busy={busy} disabled={!validAnswer || busy} data-audit="waitingAnswer">Answer</Button> : sendBack ? <>
        <Button disabled={busy} onClick={() => setSendBack(false)}>Cancel</Button>
        <Button variant="primary" disabled={busy || note.trim() === ''} busy={busy} data-audit="waitingSendBackSubmit"
          onClick={() => void run(() => props.onSendBack(task, note.trim()))}>Send back</Button>
      </> : <>
        <Button disabled={busy} data-audit="waitingSendBack" onClick={() => setSendBack(true)}>Send back</Button>
        <Button variant="primary" busy={busy} disabled={busy} data-audit="waitingAccept" onClick={() => void run(() => props.onAccept(task))}>Accept</Button>
      </>}
    </>
  )
  return (
    <>
      <Modal suspended={!open || props.taskOpen || viewing !== null} onDismiss={onClose}>
        <Dialog title={item.title} lead={item.label} width="wide" onClose={onClose} data-audit="waitingDecisionDialog" actions={actions}
          onKeyDown={(e) => { if (e.key === 'Escape') { e.stopPropagation(); onClose() } }}>
          <div className={styles.content}>
            {(error || request?.deliveryError) && <p role="alert" className={styles.error}>{error ?? request?.deliveryError}</p>}
            {resolved && request ? <SavedDecision request={request} /> : move ? <>
              <p className={styles.message}>{request?.message}</p>
              <p>Move selected tasks into <Code>{move.milestone}</Code>. Inbox tasks become Todo.</p>
              <List label="Proposed task moves">
                {move.tasks.map((t) => <ListItem key={t.id} top={<Code>{t.id}</Code>} title={t.title}
                  meta={`${t.status} → ${t.status === 'inbox' ? 'todo' : t.status} · ${t.parent ?? 'No parent'} → ${move.milestone}`}
                  foot={<div className={styles.rowActions}><Checkbox label={`Include ${t.id}`} checked={checked.includes(t.id)} disabled={busy}
                    onChange={(value) => setChecked((now) => value ? [...now, t.id] : now.filter((id) => id !== t.id))} />
                    <Button variant="link" size="sm" onClick={(e) => openTask(t.id, e.currentTarget)}>View task</Button></div>} />)}
              </List>
              <Textarea rows={2} value={note} disabled={busy} aria-label="Feedback on the proposed moves" placeholder="Optional feedback" onChange={(e) => setNote(e.target.value)} />
            </> : questions !== null ? <GroupedQuestions questions={questions} formId={formId} busy={busy} onValidityChange={setValidAnswer}
              onAnswer={(answers) => run(() => respond({ kind: 'answers', answers }))} /> : question !== null ? (
              <QuestionAnswer key={JSON.stringify(question)} task={task || item.id} title={item.title} question={question} embedded formId={formId} onValidityChange={setValidAnswer}
                busy={busy} onOpenTask={openTask} onViewImage={setViewing} previews={props.previews} onRequestPreview={props.onRequestPreview}
                onAnswer={async (id, answer) => {
                  if (request) await respond({ kind: 'answer', text: answer.text, selectedIds: answer.selectedIds ?? [] })
                  else await props.onAnswer(id, answer)
                  onClose()
                }} />
            ) : <>
              <TaskMarkdown text={props.reports[task] ?? 'Loading the latest report…'} />
              {(props.reportAttachments?.[task] ?? []).map((attachment) => attachment.kind === 'image' ? <QuestionImage key={attachment.id} task={task} image={attachment.id} title={attachment.name}
                preview={props.previews?.[attachment.id]} onRequest={props.onRequestPreview} onView={() => setViewing({ images, current: attachment.id,
                  returnFocus: document.activeElement instanceof HTMLElement ? document.activeElement : null })} /> : <p key={attachment.id}>{attachment.name}</p>)}
              {sendBack && <Textarea autoFocus rows={3} aria-label={`Why ${task} goes back`} value={note} onChange={(e) => setNote(e.target.value)} placeholder="What needs changing" disabled={busy} />}
            </>}
            <Disclosure title="Details and related tasks">
              <div className={styles.details}>
                {request && <p className={styles.message}>{request.origin.title} · {request.origin.harness} · {request.wait ? 'Waiting for your decision' : 'Independent work may continue'}</p>}
                <div className={styles.relatedActions}>
                  {request && props.onOpenSession && <Button variant="link" onClick={() => props.onOpenSession?.(request.origin.id)}>Open original conversation</Button>}
                  {task && <Button variant="link" data-audit="waitingDialogOpenTask" onClick={(e) => openTask(task, e.currentTarget)}>Open task {task}</Button>}
                  {move && <Button variant="link" onClick={(e) => openTask(move.goal, e.currentTarget)}>Open milestone task {move.goal}</Button>}
                </div>
              </div>
            </Disclosure>
          </div>
        </Dialog>
      </Modal>
      {viewing && <ImageViewer {...viewing} images={images} previews={props.previews ?? {}} closeLabel="Back"
        onClose={() => setViewing(null)} onStep={(current) => setViewing((gallery) => gallery && { ...gallery, current })}
        onRequestPreview={(image) => props.onRequestPreview?.(task, image)} />}
    </>
  )
}

function SavedDecision({ request }: { request: UserRequest }) {
  const response = request.response
  const proposal = request.request
  if (response == null) return null
  const selected = response.kind === 'answer' && proposal.kind === 'question'
    ? response.selectedIds.map((id) => typeof proposal.question === 'string' ? id : proposal.question.options.find((option) => option.id === id)?.title ?? id)
    : response.kind === 'approve' && proposal.kind === 'moveTasks'
      ? response.selectedIds.map((id) => `${id} · ${proposal.tasks.find((task) => task.id === id)?.title ?? ''}`)
      : []
  const text = response.kind === 'answer' ? response.text : response.kind === 'approve' || response.kind === 'reject' ? response.note : ''
  const batch = response.kind === 'answers' && proposal.kind === 'questions' ? response.answers.map((answer) => {
    const q = proposal.questions.find((q) => q.id === answer.id)
    const options = typeof q?.question === 'string' ? [] : q?.question.options ?? []
    return `${q?.header ?? answer.id}: ${[...answer.selectedIds.map((id) => options.find((o) => o.id === id)?.title ?? id), answer.text].filter(Boolean).join(', ')}`
  }) : []
  return (
    <div className={styles.details}>
      <p className={styles.message} role="status">{request.applying
        ? request.request.kind === 'moveTasks' ? 'Applying the approved moves…' : 'Saving your answer on the task…'
        : request.wait ? 'Your decision is saved. Sending it to the original conversation…'
          : 'Your decision is saved. The agent continued working without pausing; cide will deliver it when the current turn ends.'}</p>
      <div data-audit="waitingSavedDecision">
        <p className={styles.savedLabel}>{response.kind === 'answer' || response.kind === 'answers' ? 'Your answer' : response.kind === 'approve' ? 'Approved tasks' : response.kind === 'reject' ? 'Proposal rejected' : 'Request cancelled'}</p>
        {batch.length > 0 && <ul className={styles.savedChoices}>{batch.map((text, i) => <li key={i}>{text}</li>)}</ul>}
        {selected.length > 0 && <ul className={styles.savedChoices}>{selected.map((label, index) => <li key={index}>{label}</li>)}</ul>}
        {text !== '' && <p className={styles.message}>{text}</p>}
      </div>
    </div>
  )
}
