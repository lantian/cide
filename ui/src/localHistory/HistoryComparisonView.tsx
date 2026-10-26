import type { ReactNode } from 'react'
import type { HistoryComparison, LocalRevision } from '@/ipc/generated'
import { Banner, EmptyState, Spinner } from '@/kit/components/Feedback'
import { Button } from '@/kit/components/Button'
import { Select } from '@/kit/components/Select'
import { Toolbar } from '@/kit/components/Surface'
import { historyTime } from './model'
import styles from './LocalHistoryPanel.module.css'
export function HistoryComparisonView({ comparison, loading, error, revisions, other, onOther, canRestore, onRestore, restoring, diff }: {
  diff?: ReactNode; comparison: HistoryComparison | null; loading: boolean; error: string | null; revisions: LocalRevision[]; other: string | null; onOther: (value: string | null) => void; canRestore: boolean; onRestore: () => void; restoring: boolean
}) {
  return <>
    <Toolbar label="Revision comparison">
      <Select size="sm" aria-label="Compare with" value={other ?? 'current'} options={[{ value: 'current', label: 'Current disk file' }, ...revisions.map((r) => ({ value: r.id, label: `${historyTime(r.observedMs)} · ${r.id.slice(0, 8)}` }))]} onChange={(value) => onOther(value === 'current' ? null : value)} />
      <Button size="sm" disabled={!canRestore || restoring} busy={restoring} onClick={onRestore} title={other === null ? 'Restore the selected revision after confirmation' : 'Compare with the current disk file before restoring'}>Restore…</Button>
    </Toolbar>
    {loading ? <EmptyState icon="archive" title="Loading comparison"><Spinner /></EmptyState> : error ? <Banner tone="bad">{error}</Banner> : comparison === null ? <EmptyState icon="archive" title="Choose a revision">Comparisons are read-only and never dirty an editor buffer.</EmptyState> : <>
      <div className={styles.caption}>Selected {comparison.left.revision?.slice(0, 8)} · {comparison.left.observedMs === null ? '' : historyTime(comparison.left.observedMs)} → {comparison.right.revision ? `Revision ${comparison.right.revision.slice(0, 8)}` : 'Current disk'} · {comparison.right.observedMs === null ? '' : historyTime(comparison.right.observedMs)}</div>
      {comparison.left.content.kind === 'absent' && <Banner>The selected revision records absence. Restoring it deletes the current file.</Banner>}
      {comparison.right.content.kind === 'absent' && !comparison.right.unavailable && <Banner>The comparison’s right side is absent.</Banner>}
      {comparison.left.unavailable ? <Banner tone="bad">Selected revision unavailable: {comparison.left.unavailable}</Banner> : <>
        {comparison.right.unavailable && <Banner tone="warn">The other side is unavailable: {comparison.right.unavailable}. Showing the selected revision.</Banner>}
        {diff}
      </>}
    </>}
  </>
}
