import { spec, type ProjectId } from '@/ipc/client'
import { useWorkspace } from '@/store/workspace'
import { useTasks } from '../tasksStore'
import { startSession } from '../OpenSpecPanel/specRuns'

/** Leave the task modal, reuse/open its change tab, and wait until that tab is focused. */
export async function openTaskSpec(project: ProjectId, change: string): Promise<void> {
  useTasks.getState().select(null)
  const tab = await spec.openTab(project, { kind: 'change', change: change as never })
  await useWorkspace.getState().activateTab(project, tab)
}

/** Explicit approval always presents Apply, independent of the background-proposal setting.
 * Rust starts/reuses the change's run and marks linked Todo tasks Doing without a status echo
 * launching a second Apply. Close this card only after launch succeeds, before opening its tab. */
export async function approveTaskSpec(project: ProjectId, task: string, change: string): Promise<void> {
  const harness = useWorkspace.getState().boot?.workspace.settings.consoleHarness ?? 'claude'
  const store = useTasks.getState()
  if (store.project === project && store.board.kind === 'ready' &&
      store.board.tasks.some((row) => row.id === task && row.status === 'inbox')) {
    // Approval promotes an inbox item into work. Todo → Doing remains the launcher's one write.
    await store.edit(task, { kind: 'setStatus', status: 'todo' })
  }
  await startSession(project, {
    op: 'apply', change: change as never, launcher: { kind: 'harness', harness },
  }, () => {
    const current = useTasks.getState()
    if (current.project === project && current.selected === task) current.select(null)
  })
}
