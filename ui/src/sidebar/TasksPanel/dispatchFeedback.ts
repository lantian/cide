import type { TaskDispatchStatus } from '@/ipc/generated'

export function dispatchNotice(status: TaskDispatchStatus) {
  const heading = status.state === 'queued' ? 'Agent queued'
    : status.state === 'paused' ? 'Agent paused'
    : status.state === 'failed' ? 'Agent startup failed' : 'Agent not started'
  const summaries = status.reasons.map((reason) => {
    if (reason.code === 'dependency' && reason.task) return `Blocked by ${reason.task}`
    if (reason.code === 'inactiveMilestone' && reason.milestone) return `Milestone ${reason.milestone} is inactive`
    return reason.text
  })
  return {
    text: `${status.task} · ${status.label ?? status.agent ?? 'Agent'}: ${heading}`,
    hint: summaries.join(' · '),
    detail: status.reasons.map((reason) => reason.text).join('\n'),
    project: String(status.project),
    kind: status.state === 'failed' ? 'error' as const : 'warn' as const,
  }
}
