import type { LocalHistoryView, ProjectId } from '@/ipc/generated'
import { EmptyState } from '@/kit/components/Feedback'
export function LocalHistoryPanel({ view }: { project: ProjectId; view: LocalHistoryView }) {
  return <EmptyState icon="archive" title="Local History">{view.path ?? 'Project history'}</EmptyState>
}
