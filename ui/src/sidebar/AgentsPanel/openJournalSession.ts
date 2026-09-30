/**
 * *Open* on a Sessions tab row: reveal the pane showing the conversation, mirror a child still
 * running with no pane, or put the harness back on an ended conversation. (M134)
 *
 * Rust answers what Open means (`sessions_open`, the same three answers as `agents_run_open`, and
 * for a run the registry still holds it *is* that command's answer); `openRun.ts`'s
 * `showOpenPlan` builds the pane, which is where the spawn plan that decides who owns the child is
 * recorded — that file's header says why it cannot be the command's job.
 *
 * Loaded with a dynamic `import()` by the host, like `openRun`, so the Sessions view and its
 * render check never reach the workspace store or xterm.
 */
import { sessionJournal, type ProjectId } from '@/ipc/client'
import { showOpenPlan } from './openRun'
import { displayTitle, type SessionView } from './sessionsModel'

export async function openJournalSession(project: ProjectId, view: SessionView): Promise<void> {
  const plan = await sessionJournal.open(project, view.id)
  if (plan.kind === 'unavailable') throw new Error(plan.reason)
  await showOpenPlan(plan, displayTitle(view), 'session')
}
