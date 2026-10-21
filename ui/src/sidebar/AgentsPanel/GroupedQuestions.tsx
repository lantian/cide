import { useEffect, useState } from 'react'
import type { UserRequestAnswer, UserRequestQuestion } from '@/ipc/generated'
import { VisualChoices } from '@/kit/components/Choice'
import { Textarea } from '@/kit/components/Field'
import { TaskReferences } from '@/sidebar/TasksPanel/TaskReferences'
import styles from '@/sidebar/TasksPanel/QuestionAnswer.module.css'

/** One native tool invocation: submit the complete batch, keeping drafts on failure. */
export function GroupedQuestions({ questions, formId, busy, onValidityChange, onAnswer }: {
  questions: readonly UserRequestQuestion[]
  formId: string
  busy: boolean
  onValidityChange: (valid: boolean) => void
  onAnswer: (answers: UserRequestAnswer[]) => Promise<void>
}) {
  const [drafts, setDrafts] = useState<Record<string, UserRequestAnswer>>({})
  const answers = questions.map((q) => drafts[q.id] ?? { id: q.id, text: '', selectedIds: [] })
  const valid = answers.every((answer) => answer.selectedIds.length > 0 || answer.text.trim() !== '')
  useEffect(() => { onValidityChange(valid && !busy) }, [valid, busy, onValidityChange])
  const update = (answer: UserRequestAnswer) => setDrafts((now) => ({ ...now, [answer.id]: answer }))
  const submit = () => { if (valid && !busy) void onAnswer(answers.map((a) => ({ ...a, text: a.text.trim() }))) }
  return <form id={formId} className={styles.form} data-audit="nativeQuestionsForm"
    onSubmit={(event) => { event.preventDefault(); submit() }}>
    {questions.map((q, index) => {
      const answer = answers[index]!
      const question = q.question
      const options = typeof question === 'string' ? [] : question.options
      return <section key={q.id} aria-label={q.header} className={styles.form}>
        <strong>{q.header}</strong>
        <p className={styles.text}><TaskReferences text={typeof question === 'string' ? question : question.text} /></p>
        {options.length > 0 && <VisualChoices label={q.header} name={`${formId}-${q.id}`}
          multiple={typeof question !== 'string' && question.selection === 'multiple'}
          options={options.map((o) => ({ id: o.id, title: o.title, ...(o.description === undefined ? {} : { description: o.description }) }))}
          values={answer.selectedIds} disabled={busy} onChange={(selectedIds) => update({ ...answer, selectedIds })} />}
        {q.custom && <Textarea aria-label={`Your answer to ${q.header}`} rows={3} value={answer.text} disabled={busy}
          placeholder={options.length > 0 ? 'Custom answer or additional details' : 'Your answer'}
          onChange={(event) => update({ ...answer, text: event.target.value })}
          onKeyDown={(event) => { if (event.key === 'Enter' && (event.ctrlKey || event.metaKey)) { event.preventDefault(); submit() } }} />}
      </section>
    })}
  </form>
}
