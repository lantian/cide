import type { TaskDispatchStatus } from '@/ipc/generated'
import { Card } from '@/kit/components/Surface'
import { Button } from '@/kit/components/Button'
import styles from './TasksPanel.module.css'

export function TaskDispatchInfo({ status, error, onOpenTask, onOpenMilestone, onOpenRun, openable = false }: {
  status?: TaskDispatchStatus | null | undefined
  error?: string | null | undefined
  onOpenTask?: ((task: string) => void) | undefined
  onOpenMilestone?: ((milestone: string) => void) | undefined
  onOpenRun?: ((run: string) => void) | undefined
  openable?: boolean | undefined
}) {
  if (error) return <p className={styles.assigneeHint} role="status">Unable to check agent startup: {error}</p>
  if (!status || ((status.state === 'running' || status.state === 'idle') && status.reasons.length === 0)) return null
  const heading = {
    ready: 'Can be started', blocked: 'Agent not started', queued: 'Agent queued',
    running: 'Agent waiting', idle: 'Agent idle', paused: 'Agent paused', failed: 'Agent startup failed',
  }[status.state] ?? 'Agent startup'
  return (
    <section className={styles.dispatchInfo} data-audit="taskDispatchInfo" aria-label="Agent startup" role="status">
      <Card title={`${heading} · ${status.label ?? status.agent ?? 'Unassigned'}`}
        tone={status.state === 'failed' ? 'bad' : status.state === 'ready' ? undefined : 'warn'}>
        {status.state === 'ready' && <p>Assign the role again or @mention it to start work.</p>}
        {status.reasons.length > 0 && <ul className={styles.dispatchReasons}>
          {status.reasons.map((reason, index) => <li key={`${reason.code}:${index}`}>
            <span>{reason.text}</span>
            {reason.task && onOpenTask && <Button size="sm" onClick={() => onOpenTask(reason.task!)}>Open {reason.task}</Button>}
            {reason.milestone && onOpenMilestone && <Button size="sm" onClick={() => onOpenMilestone(reason.milestone!)}>Open {reason.milestone}</Button>}
          </li>)}
        </ul>}
        {status.run && openable && status.state !== 'queued' && onOpenRun && <Button size="sm" onClick={() => onOpenRun(status.run!)}>Open run</Button>}
      </Card>
    </section>
  )
}
