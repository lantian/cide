import { renderToStaticMarkup } from 'react-dom/server'
import type { HistoryStatus, LocalRevision, HistoryComparison } from '@/ipc/generated'
import { HistoryBrowserView, type HistoryBrowserProps } from './HistoryBrowserView'
import { HistoryComparisonView } from './HistoryComparisonView'
import { HistoryPolicyForm } from './HistoryPolicyForm'
const status: HistoryStatus = { root: 'root', canonicalRoot: '/project/without-git', generation: 1, revision: 3, usedBytes: 4096, policy: { enabled: true, retentionDays: 7, budgetBytes: 268435456, exclusions: [] }, coverage: { baselineRunning: false, scannedFiles: 2, capturedFiles: 2, notices: [], degraded: null } }
const revision: LocalRevision = { id: 'retained-a', sequence: 1, observedMs: 1700000000000, path: 'new.rs', lifecycle: 'lifecycle-a', source: 'external', priorPath: 'old.rs', content: { kind: 'content', digest: 'digest', bytes: 1 }, unavailable: null }
const props: HistoryBrowserProps = { status, roots: [status], projectBrowser: true, path: 'new.rs', paths: { items: [{ path: 'new.rs', deleted: true, latest: 'retained-a', excluded: null }], next: 'new.rs', loading: false, error: null }, revisions: { items: [revision], next: 1, loading: false, error: null }, selected: 'retained-a', filter: '', deleted: true, onFilter() {}, onDeleted() {}, onRoot() {}, onProject() {}, onPath() {}, onRevision() {}, onMorePaths() {}, onMoreRevisions() {}, onRefresh() {} }
const comparison: HistoryComparison = { left: { revision: 'retained-a', observedMs: 1700000000000, content: revision.content, text: 'A', unavailable: null }, right: { revision: null, observedMs: 1700000001000, content: { kind: 'absent' }, text: null, unavailable: null }, token: { generation: 1, path: 'new.rs', canonicalTarget: '/project/without-git/new.rs', current: { kind: 'absent' }, target: 'retained-a', lifecycle: 'lifecycle-a' } }
export function renderHistoryStories(): Record<string, string> {
  const draw = (changes: Partial<HistoryBrowserProps>) => renderToStaticMarkup(<HistoryBrowserView {...props} {...changes} />)
  return {
    ready: draw({}),
    empty: draw({ revisions: { items: [], next: null, loading: false, error: null } }),
    loading: draw({ revisions: { items: [], next: null, loading: true, error: null } }),
    excluded: draw({ revisions: { ...props.revisions, excluded: 'Excluded by Git ignore rules' } }),
    degraded: draw({ status: { ...status, coverage: { ...status.coverage, degraded: 'Queue overflow; reconciling', baselineRunning: true, notices: ['Root was closed'] } } }),
    error: draw({ revisions: { items: [], next: null, loading: false, error: 'History storage unavailable' } }),
    comparison: renderToStaticMarkup(<HistoryComparisonView comparison={comparison} loading={false} error={null} revisions={[revision]} other={null} onOther={() => {}} canRestore={true} onRestore={() => {}} restoring={false} />),
    unavailable: renderToStaticMarkup(<HistoryComparisonView comparison={{ ...comparison, left: { ...comparison.left, unavailable: 'Missing blob' } }} loading={false} error={null} revisions={[revision]} other={null} onOther={() => {}} canRestore={false} onRestore={() => {}} restoring={false} />),
    settings: renderToStaticMarkup(<HistoryPolicyForm status={status} onSave={async () => {}} onClear={() => {}} onCancel={() => {}} />),
  }
}
