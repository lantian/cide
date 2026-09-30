/**
 * The Agents panel's *Waiting for you* tab. (M132)
 *
 * It was the Tasks panel's third tab when it landed, and moved here (see `agentsTabStore.ts` for
 * why): what it lists is the far end of the runs this panel draws, and the run row's "Waiting for
 * your answer" line is a link to it. The file kept the Tasks panel's stylesheet — the rows are
 * task rows, and `TasksPanel.module.css` is where their `wait*` rules and the panel frame live;
 * the frame is the same kit `PanelHeader` the Agents panel tops with, so the header does not jump
 * when the tab changes.
 *
 * Everything on the board that cide will not move until the user does: work the user accepts by
 * eye (`acceptance: user`) that verify has passed and cide has merged, now in review; and tasks
 * with an open `question`, which the dispatcher skips while it is set. Before M132 both were
 * comments somewhere in a log — a run asking "which of these two sprites?" in a card nobody had
 * open, and the project sitting idle behind it with nothing on screen saying why.
 *
 * # A view with no store and no IPC
 *
 * `TasksPanel.tsx`'s rule and for its reason: every fact arrives as a prop and every gesture
 * leaves as one, so a server render can draw it. `AgentsPanelHost` owns the rest — the board, the
 * latest report of each row (`task_get`, since the board carries counts and not comments), and
 * the `task_respond` calls.
 *
 * # One gesture per row, and the answers are not edits
 *
 * Accept, Send back and Answer go through `tasks.respond`, never `tasks.edit`: send back and
 * answer **continue the role's own run** (`hand_back_to_run`), so the run that made the work reads
 * why in the conversation that made it. A status segment click would move the task and leave
 * that run to find out from a fresh dispatch.
 *
 * The drafts are `useState` here, the gesture state ADR 0002 leaves to the webview. A question's
 * answer box is always open — answering is the only thing to do with a question, so a button
 * that opens the box would be a click spent on nothing — and each question keeps its own draft.
 * Send back opens its box on demand, one row at a time: it sits beside Accept, and a box open
 * under every row would make the list read as a form to fill in rather than work to judge.
 */
import { useState, type ReactNode } from 'react'

import { Button } from '@/kit/components/Button'
import { TextInput, Textarea } from '@/kit/components/Field'
import { Dialog } from '@/kit/components/Overlay'
import { Modal } from '@/overlays/ModalShell'
import { List, ListItem, Section } from '@/kit/components/Surface'
import { Code } from '@/kit/components/Status'

import type { TaskView } from '@/sidebar/TasksPanel/model'
import panelStyles from '../TasksPanel/TasksPanel.module.css'

export interface WaitingPanelProps {
  project: string | null
  /** The tab strip (`AgentsPanelTabs`), drawn where the header's title goes. */
  title: ReactNode
  /** `waitingFor(board).review`: user-accepted tasks in review. */
  review: readonly TaskView[]
  /** `waitingFor(board).questions`: tasks with an open question. */
  questions: readonly TaskView[]
  /**
   * Task id → the first line of its newest report that was not superseded (`latestReport`).
   * `undefined` for a task whose log has not been read yet — drawn as nothing rather than as
   * "no report", which would be a claim about a log nobody has looked at.
   */
  reports: Readonly<Record<string, string | null>>
  /** Task ids with a response in flight; their buttons go busy. */
  busy: ReadonlySet<string>
  onOpenTask: (task: string) => void
  onAccept: (task: string) => void
  onSendBack: (task: string, note: string) => void
  onAnswer: (task: string, text: string) => void
}

type Draft = { task: string; text: string }

export function WaitingPanel({
  project,
  title,
  review,
  questions,
  reports,
  busy,
  onOpenTask,
  onAccept,
  onSendBack,
  onAnswer,
}: WaitingPanelProps) {
  const [draft, setDraft] = useState<Draft | null>(null)
  const [answers, setAnswers] = useState<Readonly<Record<string, string>>>({})
  /*
   * The question being read in full, by task id. The row shows three lines: a run's question
   * names files and payloads and can run to a paragraph, and a sidebar that grows to fit it
   * pushes every other question off screen — so the row is a preview and the dialog is where it
   * is read and answered. The draft is shared with the row's box, so nothing typed is lost by
   * opening or closing it.
   */
  const [reading, setReading] = useState<string | null>(null)
  const open = reading === null ? undefined : questions.find((t) => t.id === reading)
  const total = new Set([...review, ...questions].map((t) => t.id)).size

  // A send back with no reason is a status change that tells the run nothing, and an empty
  // answer is not an answer. Rust refuses both too; refusing here keeps the box and its text.
  const sendBack = () => {
    if (draft === null || draft.text.trim() === '') return
    onSendBack(draft.task, draft.text.trim())
    setDraft(null)
  }
  const answer = (task: string) => {
    const text = (answers[task] ?? '').trim()
    if (text === '') return
    onAnswer(task, text)
    setAnswers(({ [task]: _sent, ...rest }) => rest)
  }

  const titleButton = (t: TaskView) => (
    <Button variant="link" size="sm" data-audit="waitingOpen" onClick={() => onOpenTask(t.id)}>
      {t.title || t.id}
    </Button>
  )

  return (
    <aside className={panelStyles.panel} data-audit="sidebarWaiting" aria-label="Waiting for you">
      <div className={panelStyles.header} data-audit="agentsHeader">
        <span className={panelStyles.headerTitle}>{title}</span>
        <span className={panelStyles.headerMeta}>{total === 0 ? '' : total}</span>
      </div>
      <div className={panelStyles.body} data-audit="waitingBody">
        {project === null ? (
          <p className={panelStyles.waitQuiet}>Open a project to see what waits for you.</p>
        ) : total === 0 ? (
          <p className={panelStyles.waitQuiet} data-audit="waitingEmpty">
            Nothing is waiting for you.
          </p>
        ) : (
          <>
            {review.length > 0 && (
              <Section caption="To accept" aside={review.length}>
                <List label="Tasks to accept">
                  {review.map((t) => {
                    const report = reports[t.id]
                    return (
                      <ListItem
                        key={t.id}
                        top={<Code>{t.id}</Code>}
                        title={titleButton(t)}
                        meta={
                          report === undefined ? undefined : (
                            <span className={panelStyles.waitReport} data-audit="waitingReport">
                              {report ?? 'No report yet.'}
                            </span>
                          )
                        }
                        foot={
                          draft !== null && draft.task === t.id ? (
                            <span className={panelStyles.waitDraft}>
                              <TextInput
                                size="sm"
                                aria-label={`Why ${t.id} goes back`}
                                placeholder="Why it goes back"
                                value={draft.text}
                                autoFocus
                                onChange={(e) => setDraft({ task: t.id, text: e.target.value })}
                                onKeyDown={(e) => {
                                  if (e.key === 'Enter') {
                                    e.preventDefault()
                                    sendBack()
                                  } else if (e.key === 'Escape') {
                                    e.stopPropagation()
                                    setDraft(null)
                                  }
                                }}
                              />
                              <Button
                                size="sm"
                                variant="primary"
                                data-audit="waitingSendBackSubmit"
                                disabled={draft.text.trim() === ''}
                                onClick={sendBack}
                              >
                                Send back
                              </Button>
                            </span>
                          ) : (
                            <span className={panelStyles.waitActions}>
                              <Button
                                size="sm"
                                variant="primary"
                                data-audit="waitingAccept"
                                busy={busy.has(t.id)}
                                onClick={() => onAccept(t.id)}
                              >
                                Accept
                              </Button>
                              <Button
                                size="sm"
                                variant="secondary"
                                data-audit="waitingSendBack"
                                disabled={busy.has(t.id)}
                                onClick={() => setDraft({ task: t.id, text: '' })}
                              >
                                Send back
                              </Button>
                            </span>
                          )
                        }
                      />
                    )
                  })}
                </List>
              </Section>
            )}
            {questions.length > 0 && (
              <Section caption="Questions" aside={questions.length}>
                <List label="Open questions">
                  {questions.map((t) => (
                    <ListItem
                      key={t.id}
                      top={<Code>{t.id}</Code>}
                      title={titleButton(t)}
                      meta={
                        <span className={panelStyles.waitQuestionWrap}>
                          <span className={panelStyles.waitQuestion} data-audit="waitingQuestion">
                            {t.question}
                          </span>
                          <Button
                            variant="link"
                            size="sm"
                            data-audit="waitingReadAll"
                            onClick={() => setReading(t.id)}
                          >
                            Read in full
                          </Button>
                        </span>
                      }
                      foot={
                        <span className={panelStyles.waitDraft}>
                          <TextInput
                            size="sm"
                            aria-label={`Your answer to ${t.id}`}
                            placeholder="Your answer"
                            value={answers[t.id] ?? ''}
                            onChange={(e) => {
                              const text = e.target.value
                              setAnswers((all) => ({ ...all, [t.id]: text }))
                            }}
                            onKeyDown={(e) => {
                              if (e.key === 'Enter') {
                                e.preventDefault()
                                answer(t.id)
                              }
                            }}
                          />
                          <Button
                            size="sm"
                            variant="primary"
                            data-audit="waitingAnswer"
                            busy={busy.has(t.id)}
                            disabled={(answers[t.id] ?? '').trim() === ''}
                            onClick={() => answer(t.id)}
                          >
                            Answer
                          </Button>
                        </span>
                      }
                    />
                  ))}
                </List>
              </Section>
            )}
          </>
        )}
      </div>
      {open !== undefined && (
        <Modal onDismiss={() => setReading(null)}>
          <Dialog
            title={open.title || open.id}
            titleAside={
              /* The id opens the task's card, over this dialog's place: the card has the
                 whole story (body, reports, attachments) a question is usually about. */
              <Button
                variant="link"
                size="sm"
                data-audit="waitingDialogOpenTask"
                title="Open this task"
                onClick={() => {
                  setReading(null)
                  onOpenTask(open.id)
                }}
              >
                <Code>{open.id}</Code>
              </Button>
            }
            width="picker"
            data-audit="waitingQuestionDialog"
            onClose={() => setReading(null)}
            onKeyDown={(e) => {
              if (e.key === 'Escape') {
                e.stopPropagation()
                setReading(null)
              }
            }}
            actions={
              <>
                <Button size="sm" onClick={() => setReading(null)}>
                  Close
                </Button>
                <Button
                  size="sm"
                  variant="primary"
                  busy={busy.has(open.id)}
                  disabled={(answers[open.id] ?? '').trim() === ''}
                  onClick={() => {
                    answer(open.id)
                    setReading(null)
                  }}
                >
                  Answer
                </Button>
              </>
            }
          >
            <p className={panelStyles.waitQuestionFull}>{open.question}</p>
            <Textarea
              aria-label={`Your answer to ${open.id}`}
              placeholder="Your answer"
              rows={4}
              autoFocus
              value={answers[open.id] ?? ''}
              onChange={(e) => {
                const text = e.target.value
                setAnswers((all) => ({ ...all, [open.id]: text }))
              }}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
                  e.preventDefault()
                  answer(open.id)
                  setReading(null)
                }
              }}
            />
          </Dialog>
        </Modal>
      )}
    </aside>
  )
}
