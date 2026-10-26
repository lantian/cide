import { flushSync } from 'react-dom'
import type { ProjectId } from '@/ipc/client'
import { useTasks } from '../tasksStore'

/** Commit removal of task and originating decision overlays before a destination takes focus.
 * An async launch must not close a different card the user opened while it was pending. */
export function leaveTaskDetails(project: ProjectId, task?: string): void {
  const current = useTasks.getState()
  if (current.project !== project || current.selected === null || (task !== undefined && current.selected !== task)) return
  flushSync(() => current.closeDetails())
}
