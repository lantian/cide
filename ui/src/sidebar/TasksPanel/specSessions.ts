import type { SpecRunRow } from '@/ipc/generated'
import { ACTIVE_PHASES } from '@/sidebar/AgentsPanel/model'
import { sessionPhase } from '@/sidebar/OpenSpecPanel/model'

/** Workflow-turn presentation; leave the underlying child state available for run actions. */
export function taskSpecPhase(run: SpecRunRow) {
  return sessionPhase(run.state, run.turnComplete)
}

/** Apply belongs to the linked change as a whole, including runs launched from OpenSpec. */
export function taskSpecSessions(
  runs: readonly SpecRunRow[], task: string | null, change: string | null,
): readonly SpecRunRow[] {
  if (task === null) return []
  return runs.filter((run) =>
    (run.op === 'propose' && run.task === task) ||
    (run.op === 'apply' && (run.task === task || (change !== null && run.change === change))),
  )
}

/** One current indicator per workflow. A live run wins over history, then the most demanding
 * live phase; when none are live, use the registry's newest-first history. */
export function taskSpecStatuses(
  runs: readonly SpecRunRow[], task: string, change: string | null,
): readonly SpecRunRow[] {
  const linked = taskSpecSessions(runs, task, change)
  return (['propose', 'apply'] as const).flatMap((op) => {
    const candidates = linked.filter((run) => run.op === op)
    const live = candidates.filter((run) => {
      const phase = taskSpecPhase(run)
      return phase !== 'finished' && phase !== 'failed' && phase !== 'interrupted'
    })
    live.sort((a, b) => ACTIVE_PHASES.indexOf(taskSpecPhase(a)) - ACTIVE_PHASES.indexOf(taskSpecPhase(b)))
    const current = live[0] ?? candidates[0]
    return current ? [current] : []
  })
}
