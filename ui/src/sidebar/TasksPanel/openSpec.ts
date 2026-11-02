import { spec, type ProjectId } from '@/ipc/client'
import { useWorkspace } from '@/store/workspace'
import { useTasks } from '../tasksStore'
import { useApplyDialog } from '../OpenSpecPanel/applyStore'
import { leaveTaskDetails } from './taskNavigation'

/** Leave the task modal, reuse/open its change tab, and wait until that tab is focused. */
export async function openTaskSpec(project: ProjectId, change: string): Promise<void> {
  leaveTaskDetails(project)
  const tab = await spec.openTab(project, { kind: 'change', change: change as never })
  await useWorkspace.getState().activateTab(project, tab)
}

/** Explicit approval presents the same Apply dialog as the OpenSpec panel, so the plan-mode
 * choice (and subagent/main-model choice) is asked here too instead of silently skipped.
 * Rust starts/reuses the change's run and marks linked Todo tasks Doing on launch. The card is
 * closed first so the dialog is not drawn under its scrim. */
export async function approveTaskSpec(project: ProjectId, task: string, change: string): Promise<void> {
  const store = useTasks.getState()
  if (store.project === project && store.board.kind === 'ready' &&
      store.board.tasks.some((row) => row.id === task && row.status === 'inbox')) {
    // Approval promotes an inbox item into work. Todo → Doing remains the launcher's one write.
    await store.edit(task, { kind: 'setStatus', status: 'todo' })
  }
  leaveTaskDetails(project, task)
  useApplyDialog.getState().open(change)
}
