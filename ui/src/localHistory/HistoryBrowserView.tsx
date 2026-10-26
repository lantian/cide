import type { ReactNode } from 'react'
import type { HistoryPath, HistoryStatus, LocalRevision } from '@/ipc/generated'
import { Banner, EmptyState, Spinner } from '@/kit/components/Feedback'
import { Button } from '@/kit/components/Button'
import { SearchField } from '@/kit/components/Field'
import { Checkbox } from '@/kit/components/Choice'
import { Select } from '@/kit/components/Select'
import { List, ListItem, Toolbar } from '@/kit/components/Surface'
import { HISTORY_SOURCES, historyTime, type HistoryPage } from './model'
import styles from './LocalHistoryPanel.module.css'
export interface HistoryBrowserProps {
  status: HistoryStatus; roots: HistoryStatus[]; projectBrowser: boolean; path: string | null
  paths: HistoryPage<HistoryPath, string>; revisions: HistoryPage<LocalRevision, number>; selected: string | null
  filter: string; deleted: boolean; onFilter: (value: string) => void; onDeleted: (value: boolean) => void
  onRoot: (root: string) => void; onProject: () => void; onPath: (path: string) => void; onRevision: (id: string) => void
  onMorePaths: () => void; onMoreRevisions: () => void; onRefresh: () => void
  preview?: ReactNode; tools?: ReactNode; notice?: string | null
}
export function HistoryBrowserView(p: HistoryBrowserProps) {
  const { status, paths, revisions, path } = p
  return <section className={styles.panel} aria-label="Local History">
    <Toolbar label="Local History">
      <Select aria-label="History root" size="sm" value={status.root} disabled={!p.projectBrowser} options={p.roots.map((r) => ({ value: r.root, label: r.canonicalRoot }))} onChange={p.onRoot} />
      <Button size="sm" variant="quiet" onClick={p.onProject}>Project browser</Button><Button size="sm" variant="quiet" onClick={p.onRefresh}>Refresh</Button>{p.tools}
    </Toolbar>
    {status.coverage.degraded && <Banner tone="warn">Coverage degraded: {status.coverage.degraded}</Banner>}
    {!status.policy.enabled && <Banner>Capture is disabled. Previously retained revisions remain available.</Banner>}
    {status.coverage.baselineRunning && <Banner><Spinner /> Recording baseline / reconciling · {status.coverage.scannedFiles} files observed. Versions begin with each file’s earliest read.</Banner>}
    {status.coverage.notices.some((n) => n.includes('closed')) && <Banner>Reopened root: offline changes are represented by the next observation; intermediate closed-period versions are unavailable.</Banner>}
    {p.notice && <Banner tone="warn">{p.notice}</Banner>}
    <div className={styles.columns}>
      {p.projectBrowser && <section className={styles.paths} aria-label="Captured project paths">
        <div className={styles.filters}><SearchField size="sm" aria-label="Find captured paths" placeholder="Find captured paths…" value={p.filter} onChange={(e) => p.onFilter(e.currentTarget.value)} /><Checkbox label="Deleted files only" checked={p.deleted} onChange={p.onDeleted} /></div>
        <div className={styles.scroll}>
          {paths.loading && paths.items.length === 0 ? <EmptyState icon="archive" title="Loading paths"><Spinner /></EmptyState> : paths.error ? <Banner tone="bad">{paths.error}</Banner> : paths.items.length === 0 ? <EmptyState icon="archive" title="No captured paths">Files appear as the baseline observes them. Older paths can expire under retention limits.</EmptyState> : <List label="Captured paths">{paths.items.map((entry) => <ListItem key={entry.path} title={entry.path} meta={entry.deleted ? 'Deleted / absent' : entry.excluded ?? 'Retained file'} current={entry.path === path} onOpen={() => p.onPath(entry.path)} />)}</List>}
          {paths.next !== null && <Button size="sm" variant="quiet" busy={paths.loading} onClick={p.onMorePaths}>Load more paths</Button>}
        </div>
      </section>}
      <section className={styles.revisions} aria-label="File revisions">
        <div className={styles.caption} title={path ?? ''}>{path ?? 'Choose a captured path'}</div>
        {revisions.excluded && <Banner tone="warn">Capture excluded: {revisions.excluded}. Existing retained content remains until expiry or clear.</Banner>}
        <div className={styles.scroll}>
          {path === null ? <EmptyState icon="archive" title="Choose a file">Deleted files are available in the project browser.</EmptyState> : revisions.loading && revisions.items.length === 0 ? <EmptyState icon="archive" title="Loading revisions"><Spinner /></EmptyState> : revisions.error ? <Banner tone="bad">{revisions.error}</Banner> : revisions.items.length === 0 ? <EmptyState icon="archive" title="No retained revisions">This file has not been captured yet, is excluded, or its older versions have expired.</EmptyState> : <List label="Newest revisions first">{revisions.items.map((r) => <ListItem key={r.id} title={historyTime(r.observedMs)} top={HISTORY_SOURCES[r.source] ?? r.source} meta={<span title={r.id}>{r.id.slice(0, 8)} · lifecycle {r.lifecycle.slice(0, 8)}{r.content.kind === 'absent' ? ' · Deleted / absent' : ''}</span>} foot={r.unavailable ?? (r.priorPath ? `Renamed from ${r.priorPath}` : r.path !== path ? `Previously ${r.path}` : undefined)} current={r.id === p.selected} onOpen={() => p.onRevision(r.id)} />)}</List>}
          {revisions.next !== null && <Button size="sm" variant="quiet" busy={revisions.loading} onClick={p.onMoreRevisions}>Load older revisions</Button>}
        </div>
      </section>
      <section className={styles.preview} aria-label="Revision comparison">{p.preview ?? <EmptyState icon="archive" title="Choose a revision">Compare retained contents with the current file or another revision.</EmptyState>}</section>
    </div>
    <div className={styles.coverage}>Local recovery storage · external writes may coalesce · unsaved edits and closed periods are not continuously recorded.</div>
  </section>
}
