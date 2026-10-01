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
 * Send-back drafts are local gesture state; question drafts live in the shared QuestionAnswer
 * component, which keeps selections and custom text while comparing images or retrying a write.
 * Send back opens its box on demand, one row at a time: it sits beside Accept, and a box open
 * under every row would make the list read as a form to fill in rather than work to judge.
 */
import { useState, type ReactNode } from 'react'

import { Button } from '@/kit/components/Button'
import { TextInput } from '@/kit/components/Field'
import { Disclosure, List, ListItem } from '@/kit/components/Surface'
import { Code } from '@/kit/components/Status'

import type { TaskView, AttachmentPreview } from '@/sidebar/TasksPanel/model'
import { QuestionAnswer, type AnswerHandler } from '@/sidebar/TasksPanel/QuestionAnswer'
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
  onAnswer: AnswerHandler
  previews?: Readonly<Record<string, AttachmentPreview>> | undefined
  onRequestPreview?: ((task: string, image: string) => void) | undefined
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
  previews,
  onRequestPreview,
}: WaitingPanelProps) {
  const [draft, setDraft] = useState<Draft | null>(null)
  const total = new Set([...review, ...questions].map((t) => t.id)).size

  // A send back with no reason is a status change that tells the run nothing, and an empty
  // answer is not an answer. Rust refuses both too; refusing here keeps the box and its text.
  const sendBack = () => {
    if (draft === null || draft.text.trim() === '') return
    onSendBack(draft.task, draft.text.trim())
    setDraft(null)
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
              <Disclosure title="To accept" aside={review.length} defaultOpen>
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
              </Disclosure>
            )}
            {questions.length > 0 && (
              <Disclosure title="Questions" aside={questions.length} defaultOpen>
                <List label="Open questions">
                  {questions.map((t) => (
                    <ListItem
                      key={t.id}
                      top={<Code>{t.id}</Code>}
                      title={titleButton(t)}
                      foot={t.question === null ? undefined : (
                        <QuestionAnswer key={`${project}:${t.id}:${JSON.stringify(t.question)}`} task={t.id} title={t.title || t.id} question={t.question}
                          compact busy={busy.has(t.id)} previews={previews}
                          onRequestPreview={onRequestPreview} onAnswer={onAnswer} onOpenTask={onOpenTask} />
                      )}
                    />
                  ))}
                </List>
              </Disclosure>
            )}
          </>
        )}
      </div>

    </aside>
  )
}
