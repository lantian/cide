import type { UserRequest } from '@/ipc/generated'

/** Delivery may still be pending after the user has finished their part. */
export function waitingDecisionCount(requests: readonly UserRequest[], tasks: readonly { id: string }[]): number {
  const pending = requests.filter((request) => !request.delivered)
  const linked = new Set(pending.filter((request) => request.request.kind === 'question').map((request) => request.task))
  return pending.filter((request) => request.response == null).length
    + tasks.filter((task) => !linked.has(task.id)).length
}
