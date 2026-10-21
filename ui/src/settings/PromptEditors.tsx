import { useRef, useState } from 'react'
import type { OrchestrationConfig, OrchestrationPatch, ProjectId } from '@/ipc/client'
import { errorText } from '@/ipc/errorText'
import { Button } from '@/kit/components/Button'
import { Field, Textarea } from '@/kit/components/Field'
import { Banner } from '@/kit/components/Feedback'
import { InfoPara } from '@/kit/components/InfoTip'
import { Group } from './controls'
import styles from './PromptEditors.module.css'

type Prompts = Pick<OrchestrationConfig, 'taskStartPrompt' | 'workerInstructions'>
type Draft = Partial<Prompts>

// Switching Settings sections unmounts the form. Keep only unsaved writing, per project,
// outside React; clean text always comes from the fresh backend config. Nothing survives
// closing the window, and nothing here writes merely because the user leaves a field.
const DRAFTS = new Map<ProjectId, Draft>()

export function PromptEditors({
  project,
  config,
  save,
}: {
  project: ProjectId
  config: OrchestrationConfig
  save: (patch: OrchestrationPatch) => Promise<OrchestrationConfig>
}) {
  const [draft, setDraft] = useState<Draft>(() => DRAFTS.get(project) ?? {})
  const [busy, setBusy] = useState(false)
  const saving = useRef(false)
  const [error, setError] = useState<string | null>(null)
  const taskStartPrompt = draft.taskStartPrompt ?? config.taskStartPrompt
  const workerInstructions = draft.workerInstructions ?? config.workerInstructions
  const dirty =
    taskStartPrompt !== config.taskStartPrompt || workerInstructions !== config.workerInstructions

  function edit(field: keyof Prompts, text: string) {
    const next = { ...draft, [field]: text }
    if (text === config[field]) delete next[field]
    if (Object.keys(next).length === 0) DRAFTS.delete(project)
    else DRAFTS.set(project, next)
    setDraft(next)
  }

  async function commit(reset: boolean) {
    if (saving.current) return
    saving.current = true
    const submitted = draft
    const cached = DRAFTS.get(project)
    setBusy(true)
    setError(null)
    try {
      await save(
        reset
          ? { taskStartPrompt: '', workerInstructions: '' }
          : { taskStartPrompt, workerInstructions },
      )
      // An older save can finish after a section switch and a new edit. It must not erase
      // that newer cached draft, even though the component making this call has unmounted.
      if (DRAFTS.get(project) === cached) DRAFTS.delete(project)
      setDraft((current) => (current === submitted ? {} : current))
    } catch (cause: unknown) {
      setError(errorText(cause))
    } finally {
      saving.current = false
      setBusy(false)
    }
  }

  return (
    <Group title="Task dispatch prompts" info={<>
      <InfoPara>
        Saved for this project in <code>.cide/config.json</code>. Changes apply to future
        dispatches and worker launches. Role prompts are edited on the Roles tab.
      </InfoPara>
      <InfoPara>
        The task-start template supports <code>{'{task_id}'}</code> and{' '}
        <code>{'{task_title}'}</code>. Both editors support these tool placeholders, filled
        with the names used by the chosen harness:{' '}
        {['cide_task_get', 'cide_task_list', 'cide_task_update', 'cide_task_comment',
          'cide_task_assign', 'cide_task_create', 'cide_task_*', 'cide_user_request',
          'cide_user_request_result'].map((tool, i) => <span key={tool}>
            {i > 0 && ', '}<code>{`{${tool}}`}</code>
          </span>)}.
        Unknown placeholders stay literal. Added dispatch instructions and OpenSpec context
        still follow the task-start template.
      </InfoPara>
      <InfoPara>
        Cide worker instructions replace the tracker rules appended to a role’s prompt.
        Worker tabs keep their own system instructions. Empty text uses cide’s default.
        Unsaved edits survive Settings tab switches until this window closes.
      </InfoPara>
    </>}>
      <div className={styles.editors}>
        <div data-setting="Task-start template">
          <Field label="Task-start template" hint="First message for a cide task. Line breaks become spaces when saved.">
            {({ id, describedBy }) => <Textarea
              id={id}
              aria-describedby={describedBy}
              value={taskStartPrompt}
              rows={8}
              spellCheck={false}
              disabled={busy}
              onChange={(event) => edit('taskStartPrompt', event.target.value)}
            />}
          </Field>
        </div>
        <div data-setting="Cide worker instructions">
          <Field label="Cide worker instructions" hint="Tracker rules appended to role prompts. Role prompts have their own editor.">
            {({ id, describedBy }) => <Textarea
              id={id}
              aria-describedby={describedBy}
              value={workerInstructions}
              rows={12}
              spellCheck={false}
              disabled={busy}
              onChange={(event) => edit('workerInstructions', event.target.value)}
            />}
          </Field>
        </div>
        {error !== null && <Banner tone="warn">Prompts could not be saved. {error}</Banner>}
        <div className={styles.buttons}>
          <Button variant="primary" busy={busy} disabled={!dirty} onClick={() => void commit(false)}>
            Save prompts
          </Button>
          <Button disabled={busy} onClick={() => void commit(true)}>Reset to defaults</Button>
        </div>
      </div>
    </Group>
  )
}
