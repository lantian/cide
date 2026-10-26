import { useEffect, useRef, useState } from 'react'
import type { HistoryComparison, HistoryPath, HistoryStatus, LocalHistoryView, LocalRevision, ProjectId } from '@/ipc/generated'
import { events, localHistory } from '@/ipc/client'
import { EmptyState, Spinner } from '@/kit/components/Feedback'
import { Button } from '@/kit/components/Button'
import { ConfirmDestructive } from '@/chrome/ConfirmDestructive'
import { HistoryComparisonView } from './HistoryComparisonView'
import { comparisonMatches, historyTime } from './model'
import { notify, notifyFailure } from '@/chrome/notices'
import { HistoryBrowserView } from './HistoryBrowserView'
import { appendUnique, HistoryRequests, type HistoryPage } from './model'

function empty<T, C>(): HistoryPage<T, C> { return { items: [], next: null, loading: false, error: null } }
export function LocalHistoryPanel({ project, view }: { project: ProjectId; view: LocalHistoryView }) {
  const [roots, setRoots] = useState<HistoryStatus[]>([])
  const [rootError, setRootError] = useState<string | null>(null)
  const [refresh, setRefresh] = useState(0)
  const [path, setPath] = useState(view.path)
  const [selected, setSelected] = useState<string | null>(null)
  const [other, setOther] = useState<string | null>(null)
  const [comparison, setComparison] = useState<HistoryComparison | null>(null)
  const [comparing, setComparing] = useState(false)
  const [compareError, setCompareError] = useState<string | null>(null)
  const [selectionNotice, setSelectionNotice] = useState<string | null>(null)
  const [confirmation, setConfirmation] = useState<HistoryComparison | null>(null)
  const [restoring, setRestoring] = useState(false)
  const comparisonRequests = useRef(new HistoryRequests())
  const [filter, setFilter] = useState('')
  const [deleted, setDeleted] = useState(false)
  const [paths, setPaths] = useState<HistoryPage<HistoryPath, string>>(empty)
  const [revisions, setRevisions] = useState<HistoryPage<LocalRevision, number>>(empty)
  const status = roots.find((r) => r.root === view.root)
  const statusRef = useRef(status); statusRef.current = status
  const statusRequests = useRef(new HistoryRequests())
  const pathRequests = useRef(new HistoryRequests())
  const revisionRequests = useRef(new HistoryRequests())
  useEffect(() => {
    let dropped = false
    const subscription = events.onLocalHistoryChanged((change) => {
      if (dropped || change.root !== view.root) return
      pathRequests.current.cancel(); revisionRequests.current.cancel(); comparisonRequests.current.cancel(); setComparison(null); setConfirmation(null)
      if (change.generation !== statusRef.current?.generation) { setSelected(null); setRevisions(empty()); setPaths(empty()); if (view.path === null) setPath(null) }
      setRefresh((n) => n + 1)
    })
    void subscription.then(() => { if (!dropped) setRefresh((n) => n + 1) }).catch((error: unknown) => { if (!dropped) setRootError(String(error)) })
    return () => { dropped = true; void subscription.then((stop) => stop()).catch(() => {}) }
  }, [view.root])
  useEffect(() => {
    const request = statusRequests.current.begin()
    void localHistory.status(project).then((answer) => {
      if (!statusRequests.current.accepts(request)) return
      setRoots(answer); setRootError(answer.some((r) => r.root === view.root) ? null : 'This history root is no longer open.')
    }).catch((error: unknown) => { if (statusRequests.current.accepts(request)) setRootError(String(error)) })
    return () => statusRequests.current.cancel()
  }, [project, view.root, refresh])
  useEffect(() => { comparisonRequests.current.cancel(); setSelected(null); setOther(null); setComparison(null); setConfirmation(null) }, [status?.generation, path])
  useEffect(() => {
    const request = pathRequests.current.begin()
    if (!status?.generation || view.path !== null) { if (status?.coverage.degraded && !status.coverage.baselineRunning) setPaths({ ...empty(), error: status.coverage.degraded }); return }
    setPaths({ ...empty(), loading: true })
    void localHistory.paths(project, view.root, status.generation, null, filter, deleted).then((page) => {
      if (pathRequests.current.accepts(request)) setPaths({ items: page.paths, next: page.next, loading: false, error: null })
    }).catch((error: unknown) => { if (pathRequests.current.accepts(request)) setPaths({ ...empty(), error: String(error) }) })
    return () => pathRequests.current.cancel()
  }, [project, view.root, view.path, status?.generation, filter, deleted, refresh])
  useEffect(() => {
    const request = revisionRequests.current.begin()
    setRevisions({ ...empty(), loading: path !== null })
    if (!status?.generation || path === null) { if (status?.coverage.degraded && !status.coverage.baselineRunning) setRevisions({ ...empty(), error: status.coverage.degraded }); return }
    void localHistory.list(project, view.root, status.generation, path, null).then((page) => {
      if (revisionRequests.current.accepts(request)) setRevisions({ items: page.revisions, next: page.next, excluded: page.excluded, loading: false, error: null })
    }).catch((error: unknown) => { if (revisionRequests.current.accepts(request)) setRevisions({ ...empty(), error: String(error) }) })
    return () => revisionRequests.current.cancel()
  }, [project, view.root, path, status?.generation, refresh])
  useEffect(() => {
    const request = comparisonRequests.current.begin()
    setComparison(null); setCompareError(null)
    if (!status?.generation || path === null || selected === null) { setComparing(false); return }
    setComparing(true)
    void localHistory.compare(project, view.root, status.generation, path, selected, other).then((answer) => {
      if (comparisonRequests.current.accepts(request)) { setComparison(answer); setComparing(false) }
    }).catch((error: unknown) => {
      if (comparisonRequests.current.accepts(request)) { setComparing(false); setCompareError(String(error)); setSelectionNotice(String(error)); setSelected(null) }
    })
    return () => comparisonRequests.current.cancel()
  }, [project, view.root, status?.generation, path, selected, other, refresh])
  const restore = async (): Promise<void> => {
    if (confirmation === null || restoring) return
    const confirmed = confirmation
    setRestoring(true)
    try {
      const result = await localHistory.restore(project, view.root, confirmed.token, true)
      setConfirmation(null)
      setSelectionNotice(result.degraded ?? `Restored ${confirmed.token.path}. Its pre-restore state remains in Local History.`)
      notify(result.degraded ?? `Restored ${confirmed.token.path}`, { kind: result.degraded ? 'warn' : 'ok' })
    } catch (error) { setConfirmation(null); setSelectionNotice(String(error)); setComparison(null) }
    finally { setRestoring(false); setRefresh((n) => n + 1) }
  }
  const morePaths = (): void => {
    if (!status || paths.next === null || paths.loading) return
    const request = pathRequests.current.begin(); setPaths((p) => ({ ...p, loading: true }))
    void localHistory.paths(project, view.root, status.generation, paths.next, filter, deleted).then((page) => {
      if (pathRequests.current.accepts(request)) setPaths((p) => ({ ...p, items: appendUnique(p.items, page.paths, (v) => v.path), next: page.next, loading: false }))
    }).catch((error: unknown) => { if (pathRequests.current.accepts(request)) setPaths((p) => ({ ...p, loading: false, error: String(error) })) })
  }
  const moreRevisions = (): void => {
    if (!status || path === null || revisions.next === null || revisions.loading) return
    const request = revisionRequests.current.begin(); setRevisions((p) => ({ ...p, loading: true }))
    void localHistory.list(project, view.root, status.generation, path, revisions.next).then((page) => {
      if (revisionRequests.current.accepts(request)) setRevisions((p) => ({ ...p, items: appendUnique(p.items, page.revisions, (v) => v.id), next: page.next, loading: false }))
    }).catch((error: unknown) => { if (revisionRequests.current.accepts(request)) setRevisions((p) => ({ ...p, loading: false, error: String(error) })) })
  }
  if (!status) return <EmptyState icon="archive" title={rootError ? 'History root unavailable' : 'Opening Local History'} action={<Button size="sm" onClick={() => setRefresh((n) => n + 1)}>Retry</Button>}>{rootError ?? <Spinner />}</EmptyState>
  const canRestore = comparisonMatches(comparison, path, selected, other, status.generation) && status.policy.enabled && other === null && !comparison?.left.unavailable && !comparison?.right.unavailable && !comparing
  return <><HistoryBrowserView status={status} roots={roots} projectBrowser={view.path === null} path={path} paths={paths} revisions={revisions} selected={selected} filter={filter} deleted={deleted}
    onFilter={(value) => { pathRequests.current.cancel(); setFilter(value) }} onDeleted={(value) => { pathRequests.current.cancel(); setDeleted(value) }}
    onPath={(value) => { revisionRequests.current.cancel(); comparisonRequests.current.cancel(); setComparison(null); setConfirmation(null); setSelected(null); setPath(value) }} onRevision={(value) => { comparisonRequests.current.cancel(); setComparison(null); setConfirmation(null); setSelectionNotice(null); setSelected(value) }}
    onRoot={(root) => { void localHistory.openView(project, null, root).catch(notifyFailure) }} onProject={() => { void localHistory.openView(project, null, view.root).catch(notifyFailure) }}
    onMorePaths={morePaths} onMoreRevisions={moreRevisions} onRefresh={() => setRefresh((n) => n + 1)} notice={rootError ?? selectionNotice}
    preview={<HistoryComparisonView comparison={comparison} loading={comparing} error={compareError} revisions={revisions.items} other={other} onOther={(value) => { comparisonRequests.current.cancel(); setComparison(null); setConfirmation(null); setOther(value) }} canRestore={canRestore} restoring={restoring} onRestore={() => { if (canRestore) setConfirmation(comparison) }} />} />
    {confirmation !== null && <ConfirmDestructive state={{
      title: 'Restore Local History revision?',
      body: `Restore revision ${confirmation.token.target} from ${confirmation.left.observedMs === null ? 'its observation' : historyTime(confirmation.left.observedMs)}. ${confirmation.left.content.kind === 'absent' ? 'This revision records absence and will delete the current file.' : 'This replaces the current file contents.'} The current state is retained first. Dirty editors or changed disk contents will abort the restore.`,
      files: [confirmation.token.canonicalTarget], confirmLabel: 'Restore revision', run: () => { void restore() },
    }} busy={restoring} onCancel={() => { if (!restoring) setConfirmation(null) }} onConfirm={() => { void restore() }} />}
  </>
}
