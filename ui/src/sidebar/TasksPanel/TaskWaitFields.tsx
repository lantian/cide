/**
 * The card's three M132 rows: **Touches**, **Acceptance** and **Question**.
 *
 * Their own file because `TaskDetail.tsx` is past two and a half thousand lines, and because the
 * three share one rule the rest of the card does not: each is optional *structurally*. A row is
 * drawn only when its handler is present (or, for Question, when there is a question to show) —
 * the `links`/`spec` optionality claim again, so every story written before M132 renders the card
 * it always rendered and `check:agents-render` keeps pinning the same markup.
 *
 * Like the card, these read **no store and call no IPC**: every write arrives as a prop, and
 * `TaskDetailHost` maps it onto `TaskEdit`'s `setTouches` / `setAcceptance` / `setQuestion`. The
 * drafts are local `useState`, which is the transient gesture state ADR 0002 leaves to the webview
 * — a half-typed glob list reaches no other window and survives nothing, and neither should it.
 *
 * Built from the kit (`Button`, `Textarea`, `TextInput`, `Switch`, `Tag`); the frame is the
 * card's own `.field` box, so the rows sit in the card's rhythm rather than a second one.
 */
import { useState } from 'react'

import { Button } from '@/kit/components/Button'
import { Switch } from '@/kit/components/Choice'
import { Textarea, TextInput } from '@/kit/components/Field'
import { Tag } from '@/kit/components/Status'

import { parseTouches, type TaskView } from './model'
import styles from './TasksPanel.module.css'

/**
 * Touches: the globs this task will change. At rest, one tag per glob — or *Whole repository*,
 * which is what an empty list **means** (the dispatcher treats it as the widest claim), so it is
 * said in words rather than drawn as an empty row that reads as "unknown".
 *
 * Edited as text, one glob per line: a glob list is typed and pasted far more than it is picked,
 * and a chip-per-entry editor makes pasting five paths five gestures.
 */
export function TouchesField({
  task,
  onSetTouches,
}: {
  task: TaskView
  onSetTouches: (task: string, touches: string[]) => void
}) {
  const [draft, setDraft] = useState<string | null>(null)
  const save = () => {
    if (draft === null) return
    const next = parseTouches(draft)
    setDraft(null)
    // Nothing changed is nothing written: an unchanged Save would bump the board's `rev` and
    // disarm whatever the list had armed, for a gesture that meant "leave it".
    if (next.join('\n') === task.touches.join('\n')) return
    onSetTouches(task.id, next)
  }
  return (
    <div className={styles.field} data-audit="taskTouches">
      <div className={styles.fieldHead}>
        <span className={styles.fieldLabel}>Touches</span>
        <button
          type="button"
          className={styles.fieldEdit}
          data-audit={draft === null ? 'taskTouchesEdit' : 'taskTouchesCancel'}
          aria-label={draft === null ? `Edit the files ${task.id} touches` : 'Cancel'}
          title={draft === null ? 'Edit' : 'Cancel'}
          onClick={() => setDraft(draft === null ? task.touches.join('\n') : null)}
        >
          {draft === null ? 'Edit' : 'Cancel'}
        </button>
      </div>
      {draft === null ? (
        task.touches.length === 0 ? (
          <p className={`${styles.fieldValue} ${styles.fieldValueEmpty}`}>Whole repository</p>
        ) : (
          <div className={styles.touchTags}>
            {task.touches.map((glob) => (
              <Tag key={glob}>{glob}</Tag>
            ))}
          </div>
        )
      ) : (
        <>
          <Textarea
            aria-label={`Files ${task.id} touches, one glob per line`}
            value={draft}
            rows={Math.min(8, Math.max(2, draft.split('\n').length))}
            placeholder={'crates/cide-git/**\nui/src/sidebar/GitPanel/**'}
            spellCheck={false}
            autoFocus
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape') {
                e.stopPropagation()
                setDraft(null)
              } else if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
                e.preventDefault()
                save()
              }
            }}
          />
          <div className={styles.fieldActions}>
            <Button size="sm" variant="primary" data-write="true" onClick={save}>
              Save
            </Button>
          </div>
        </>
      )}
    </div>
  )
}

/**
 * Acceptance: whether the user, not a model reviewer, accepts this task. A switch, because it
 * takes effect at once (the kit's rule for `Switch` against `Checkbox`).
 */
export function AcceptanceField({
  task,
  onSetAcceptance,
}: {
  task: TaskView
  onSetAcceptance: (task: string, acceptance: 'user' | null) => void
}) {
  const on = task.acceptance === 'user'
  return (
    <div className={styles.field} data-audit="taskAcceptance" data-on={on ? 'true' : 'false'}>
      <div className={styles.fieldHead}>
        <span className={styles.fieldLabel}>Accepted by the user</span>
        <Switch
          checked={on}
          label={`${task.id} is accepted by the user`}
          onChange={(next) => onSetAcceptance(task.id, next ? 'user' : null)}
        />
      </div>
      <p className={styles.fieldHint}>
        {on
          ? 'Merged once verify passes, then it waits for you in Waiting for you.'
          : 'A reviewer judges it. Turn on for work only you can judge by eye.'}
      </p>
    </div>
  )
}

/**
 * Question: an open question this task waits on. Drawn only while one is set — asking is a run's
 * or a console's act (`cide_task_update`), and an empty "Question" row on every card would read
 * as something to fill in. Edit rewrites it; Clear takes it away, which lets the task dispatch
 * again without an answer (the Waiting tab's Answer is the road that also tells the run why).
 */
export function QuestionField({
  task,
  onSetQuestion,
  onAnswer,
}: {
  task: TaskView
  onSetQuestion?: ((task: string, question: string | null) => void) | undefined
  /**
   * Answer the question from the card (M132): the same `task_respond` the Waiting list sends —
   * the answer is written on the task, the question cleared, and the role on it continues.
   * Absent draws no box, so older stories are unchanged.
   */
  onAnswer?: ((task: string, text: string) => void) | undefined
}) {
  const [draft, setDraft] = useState<string | null>(null)
  const [answer, setAnswer] = useState('')
  if (task.question === null) return null
  const send = () => {
    const text = answer.trim()
    if (text === '') return
    onAnswer?.(task.id, text)
    setAnswer('')
  }
  const save = () => {
    if (draft === null) return
    const next = draft.trim()
    setDraft(null)
    if (next === task.question) return
    // An emptied question is a clear, said the long way round — the same answer Clear gives.
    onSetQuestion?.(task.id, next === '' ? null : next)
  }
  return (
    <div className={styles.field} data-audit="taskQuestion">
      <div className={styles.fieldHead}>
        <span className={styles.fieldLabel}>Question</span>
        {onSetQuestion !== undefined && draft === null && (
          <>
            <button
              type="button"
              className={styles.fieldEdit}
              data-audit="taskQuestionEdit"
              onClick={() => setDraft(task.question ?? '')}
            >
              Edit
            </button>
            <button
              type="button"
              className={styles.fieldEdit}
              data-audit="taskQuestionClear"
              data-write="true"
              title="Clear the question; the task can be dispatched again"
              onClick={() => onSetQuestion(task.id, null)}
            >
              Clear
            </button>
          </>
        )}
      </div>
      {draft === null ? (
        <>
          <p className={styles.fieldValue}>{task.question}</p>
          {onAnswer !== undefined && (
            <div className={styles.questionAnswer} data-audit="taskQuestionAnswer">
              <Textarea
                aria-label={`Your answer to ${task.id}`}
                placeholder="Your answer — Ctrl+Enter sends"
                rows={3}
                value={answer}
                onChange={(e) => setAnswer(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
                    e.preventDefault()
                    send()
                  }
                }}
              />
              <Button
                size="sm"
                variant="primary"
                data-write="true"
                disabled={answer.trim() === ''}
                onClick={send}
              >
                Answer
              </Button>
            </div>
          )}
        </>
      ) : (
        <div className={styles.inlineEdit}>
          <TextInput
            size="sm"
            aria-label={`The question on ${task.id}`}
            value={draft}
            autoFocus
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault()
                save()
              } else if (e.key === 'Escape') {
                e.stopPropagation()
                setDraft(null)
              }
            }}
          />
          <Button size="sm" variant="primary" data-write="true" onClick={save}>
            Save
          </Button>
          <Button size="sm" variant="quiet" onClick={() => setDraft(null)}>
            Cancel
          </Button>
        </div>
      )}
    </div>
  )
}
