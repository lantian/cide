import { useMemo, useState } from 'react'
import { Button } from '@/kit/components/Button'
import { file, type SearchSession } from '@/ipc/client'
import { jumpTo } from '@/editor/jump'
import { UNKNOWN_LINE } from '@/editor/navHistory'
import { notifyFailure } from '@/chrome/notices'
import { results, location, resultRows, type Result } from './model'
import { useSearchSessions, openSearch, refreshSession } from './store'
import { ResultsView } from './ResultsView'
import type { OpenHit } from '@/sidebar/SearchPanel'
import styles from './Search.module.css'

export function PinnedSearch({ session, onOpenHit }: { session: SearchSession; onOpenHit?: OpenHit | undefined }) {
  const frame = useSearchSessions(s => s.frames[session.id] ?? null)
  const error = useSearchSessions(s => s.errors[session.id] ?? '')
  const [refreshing, setRefreshing] = useState(false)
  const items = useMemo(() => results(frame), [frame])
  const descriptor = session.descriptor
  const query = descriptor.query
  const open = (result: Result) => {
    const at = location(result)
    if (!at) return
    if (result.kind === 'file') {
      jumpTo(session.project, { path: at.path, line: UNKNOWN_LINE, column: 1 })
      void file.open(session.project, at.path).catch(notifyFailure)
    } else if (onOpenHit) onOpenHit(at.path, at.line, at.column, at.endColumn)
    else {
      jumpTo(session.project, at)
      void file.open(session.project, at.path).catch(notifyFailure)
    }
  }
  const refresh = async () => {
    setRefreshing(true)
    try { await refreshSession(session) } catch (e) { notifyFailure(e) }
    finally { setRefreshing(false) }
  }
  const status = [error, ...(frame?.errors ?? []), frame?.text?.error?.detail].filter(Boolean).join(' · ')
  const limited = frame?.text?.truncated || (frame?.files && frame.files.matched > frame.files.items.length)
    || (frame?.symbols && (frame.symbols.truncated || frame.symbols.matched > frame.symbols.items.length))
  const summary = [query.pattern, query.mode === 'regex' ? 'Regex' : '', query.caseSensitive ? 'Match case' : '', query.wholeWord ? 'Whole word' : '', query.scope ? `Folder: ${query.scope}` : '', query.include ? `Files: ${query.include}` : '', descriptor.libraries ? 'Includes library files' : ''].filter(Boolean).join(' · ')
  return <section className={styles.pinned} aria-label={`Pinned search: ${query.pattern}`} data-audit="pinnedSearch">
    <div className={styles.toolbar}>
      <Button size="sm" icon="refresh-cw" busy={refreshing} onClick={() => void refresh()}>Refresh</Button>
      <Button size="sm" onClick={() => openSearch(descriptor.source, descriptor)}>Edit search</Button>
      <Button size="sm" variant="quiet" onClick={() => useSearchSessions.getState().patchView(session.id, { collapsed: new Set() })}>Expand all</Button>
      <Button size="sm" variant="quiet" onClick={() => useSearchSessions.getState().patchView(session.id, { collapsed: new Set(resultRows(items, descriptor.source, new Set(), true).filter(row => row.kind === 'group').map(row => row.key)) })}>Collapse all</Button>
      <span className={styles.querySummary} title={summary}>{summary}</span>
    </div>
    <ResultsView project={session.project} id={session.id} items={items} mode={descriptor.source} pinned running={frame?.running ?? true} error={status} onOpen={open} />
    <div className={styles.footer}>{frame?.running ? 'Searching…' : `${items.length.toLocaleString()} results`}{limited ? ' · Result limit reached' : ''}</div>
  </section>
}
