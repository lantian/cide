import { memo, useCallback, useEffect, useId, useMemo, useRef, useState } from 'react'
import { useVirtualizer } from '@tanstack/react-virtual'
import type { ProjectId } from '@/ipc/client'
import { Icon, type IconName } from '@/icons/Icon'
import { copyText } from '@/sidebar/copyText'
import { NO_SOURCE, type Diagnostic, type DiagnosticsSnapshot, type Severity } from '@/sidebar/ProblemsPanel/model'
import { useContextMenu } from '@/menus'
import { scaledRow, useUiScale } from '@/settings/useUiScale'
import { DEFAULT_PROBLEMS_VIEW, diagnosticIndicator, problemDescription, problemGroups, problemKey } from './problemsModel'
import { setProblemsView, useProblemsState } from './problemsState'
import styles from './ProblemsTab.module.css'

const icons: Record<Severity, IconName> = { error: 'circle-x', warning: 'triangle-alert', info: 'info', hint: 'minus' }
type Row = { key: string; path: string; label: string; item?: Diagnostic; count?: number }

export interface ProblemsTabProps {
  project: ProjectId | null
  snapshot?: DiagnosticsSnapshot | undefined
  file?: string | null | undefined
  onOpenLocation?: ((path: string, line: number, column: number) => void) | undefined
  onRefresh?: (() => void) | undefined
}

// Workspace broadcasts and the host's search polling must not rebuild the diagnostic tree.
export const ProblemsTab = memo(function ProblemsTab({ project, snapshot = NO_SOURCE, file = null, onOpenLocation, onRefresh }: ProblemsTabProps) {
  const view = useProblemsState(s => s.views[project ?? ''] ?? DEFAULT_PROBLEMS_VIEW)
  const update = (patch: Parameters<typeof setProblemsView>[1]) => setProblemsView(project, patch)
  const groups = useMemo(() => problemGroups(snapshot, view, file), [snapshot, view, file])
  const rows = useMemo((): Row[] => {
    const collapsed = new Set(view.collapsed)
    return groups.flatMap(group => [
      { key: `file:${group.path}`, path: group.path, label: group.label, count: group.items.length },
      ...(collapsed.has(group.path) ? [] : group.items.map(item => ({ key: problemKey(item), path: group.path, label: item.message, item }))),
    ])
  }, [groups, view.collapsed])
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [cursor, setCursor] = useState<string | null>(null)
  const tree = useRef<HTMLDivElement>(null)
  const rowHeight = scaledRow(25, useUiScale())
  const getRowKey = useCallback((index: number) => rows[index]!.key, [rows])
  const virtual = useVirtualizer({
    count: rows.length,
    getScrollElement: () => tree.current,
    getItemKey: getRowKey,
    estimateSize: () => rowHeight,
    overscan: 8,
  })
  const visibleRows = virtual.getVirtualItems()
  useEffect(() => { virtual.measure() }, [virtual, rowHeight])
  const anchor = useRef<string | null>(null)
  const id = useId()
  const at = Math.max(0, rows.findIndex(r => r.key === cursor))
  const current = rows[at]
  const chosen = (target?: HTMLElement | null): Diagnostic[] => {
    const targetKey = target?.closest<HTMLElement>('[data-problem-key]')?.dataset['problemKey']
    const keys = targetKey && !selected.has(targetKey) ? new Set([targetKey]) : selected
    return rows.flatMap(row => row.item && keys.has(row.key) ? [row.item] : [])
  }
  const open = (row = current) => {
    if (row?.item) onOpenLocation?.(row.item.absPath ?? row.item.path, row.item.line, row.item.column)
  }
  const openRef = useRef(open)
  openRef.current = open
  useEffect(() => {
    const listener = () => openRef.current()
    window.addEventListener('cide:problem-open', listener)
    tree.current?.focus({ preventScroll: true })
    return () => {
      window.removeEventListener('cide:problem-open', listener)
      useProblemsState.setState({ focused: false })
    }
  }, [])
  const menu = useContextMenu({
    label: 'Problem actions',
    items: invocation => {
      const items = chosen(invocation.target)
      const item = items[0]
      return [
        { id: 'source', label: 'Jump to Source', command: 'problems.jumpToSource', ...(item && onOpenLocation ? { run: () => onOpenLocation(item.absPath ?? item.path, item.line, item.column) } : { disabledReason: 'Select a problem' }) },
        { id: 'copy', label: 'Copy Problem Description', ...(items.length ? { run: () => { void copyText(items.map(problemDescription).join('\n')) } } : { disabledReason: 'Select a problem' }) },
      ]
    },
  })
  const toggle = (path: string) => update({ collapsed: view.collapsed.includes(path) ? view.collapsed.filter(p => p !== path) : [...view.collapsed, path] })
  const select = (index: number, shift = false, additive = false) => {
    const row = rows[index]
    if (!row) return
    const base = rows.findIndex(r => r.key === anchor.current)
    const keys = shift && base >= 0 ? rows.slice(Math.min(base, index), Math.max(base, index) + 1).map(r => r.key) : [row.key]
    const next = additive ? new Set(selected) : new Set<string>()
    for (const key of keys) {
      if (additive && !shift && next.has(key)) next.delete(key)
      else next.add(key)
    }
    if (!shift) anchor.current = row.key
    setCursor(row.key)
    setSelected(next)
    virtual.scrollToIndex(index, { align: 'auto' })
  }
  const allItems = useMemo(() => snapshot.kind === 'unavailable' ? [] : snapshot.items ?? [], [snapshot])
  const sources = useMemo(() => [...new Set([...allItems.map(item => item.source), view.source].filter((s): s is string => !!s))].sort(), [allItems, view.source])
  const indicator = useMemo(() => diagnosticIndicator(snapshot), [snapshot])
  const total = groups.reduce((n, group) => n + group.items.length, 0)
  const empty = project === null ? 'No project open' : view.scope === 'file' && !file ? 'No current file' : snapshot.kind === 'unavailable' ? snapshot.reason : indicator.scanning ? 'Analysis in progress…' : allItems.length > 0 ? 'No problems match this view' : indicator.partial ? 'Analysis is incomplete; some analysers are unavailable.' : 'No problems found'
  return <section className={styles.panel} aria-label="Problems" data-audit="problemsToolWindow"
    onFocusCapture={() => useProblemsState.setState({ focused: true })}
    onBlurCapture={event => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) useProblemsState.setState({ focused: false }) }}>
    <div className={styles.toolbar}>
      <div className={styles.scopes} role="tablist" aria-label="Problem scope">
        <button className={styles.scope} role="tab" aria-selected={view.scope === 'file'} onClick={() => update({ scope: 'file' })}>Current File</button>
        <button className={styles.scope} role="tab" aria-selected={view.scope === 'project'} onClick={() => update({ scope: 'project' })}>Project</button>
      </div>
      {(Object.keys(icons) as Severity[]).map(severity => <button key={severity} className={styles.button} aria-label={`Show ${severity} problems`} aria-pressed={view.severities[severity]} onClick={() => update({ severities: { ...view.severities, [severity]: !view.severities[severity] } })} title={`Show ${severity} problems`}><span className={styles[severity]}><Icon name={icons[severity]} size={1} /></span></button>)}
      <select className={styles.select} aria-label="Diagnostic source" value={view.source} onChange={e => update({ source: e.target.value })}><option value="">All sources</option>{sources.map(source => <option key={source}>{source}</option>)}</select>
      <select className={styles.select} aria-label="Sort problems" value={view.sort} onChange={e => update({ sort: e.target.value as 'position' | 'severity' })}><option value="position">Sort by position</option><option value="severity">Sort by severity</option></select>
      <button className={styles.button} onClick={() => update({ collapsed: [] })}>Expand All</button>
      <button className={styles.button} onClick={() => update({ collapsed: groups.map(g => g.path) })}>Collapse All</button>
      {onRefresh && <button className={styles.button} onClick={onRefresh}>Re-run Analysis</button>}
      <span className={styles.meta}>{total} {total === 1 ? 'problem' : 'problems'}</span>
    </div>
    {indicator.scanning && <div className={styles.note} role="status">Analysis in progress…</div>}
    {indicator.partial && snapshot.kind !== 'unavailable' && <div className={styles.note}>Some analysers are unavailable. Results may be incomplete.</div>}
    {snapshot.kind === 'ready' && !!snapshot.truncated && <div className={styles.note}>{snapshot.truncated} additional problems are not included in this snapshot.</div>}
    <div ref={tree} className={styles.tree} role="tree" aria-label="Problems" aria-multiselectable="true" aria-activedescendant={current && visibleRows.some(row => row.index === at) ? `${id}-${at}` : undefined} tabIndex={0}
      onContextMenu={menu.onContextMenu}
      onKeyDown={event => {
        if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === 'c') {
          const items = chosen()
          if (items.length) { event.preventDefault(); event.stopPropagation(); void copyText(items.map(problemDescription).join('\n')) }
          return
        }
        if (event.key === 'Enter' || event.key === 'F4') {
          event.preventDefault(); event.stopPropagation()
          if (current && !current.item && event.key === 'Enter') toggle(current.path)
          else open()
          return
        }
        if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
          event.preventDefault(); event.stopPropagation()
          if (current) {
            if (event.key === 'ArrowLeft') {
              if (current.item) select(rows.findIndex(row => row.key === `file:${current.path}`))
              else update({ collapsed: [...new Set([...view.collapsed, current.path])] })
            } else if (!current.item) {
              if (view.collapsed.includes(current.path)) update({ collapsed: view.collapsed.filter(path => path !== current.path) })
              else select(at + 1)
            }
          }
          return
        }
        const next = event.key === 'ArrowDown' ? Math.min(rows.length - 1, at + 1) : event.key === 'ArrowUp' ? Math.max(0, at - 1) : event.key === 'Home' ? 0 : event.key === 'End' ? rows.length - 1 : null
        if (next !== null) { event.preventDefault(); event.stopPropagation(); select(next, event.shiftKey) }
      }}>
      {rows.length === 0 ? <div className={styles.empty} role="status">{empty}</div> : <div className={styles.virtualRows} style={{ height: virtual.getTotalSize() }}>{visibleRows.map(virtualRow => {
        const index = virtualRow.index
        const row = rows[index]!
        return <div key={row.key} id={`${id}-${index}`} role="treeitem" aria-level={row.item ? 2 : 1} aria-selected={selected.has(row.key)} aria-expanded={row.item ? undefined : !view.collapsed.includes(row.path)}
        className={row.item ? styles.row : styles.group} style={{ height: virtualRow.size, transform: `translateY(${virtualRow.start}px)` }} data-problem-key={row.key} title={row.item ? `${problemDescription(row.item)}${row.item.stale ? '\nChanged since analysis; position may be out of date.' : ''}` : row.path}
        onClick={e => { tree.current?.focus(); select(index, e.shiftKey, e.ctrlKey || e.metaKey); if (!row.item) toggle(row.path) }} onDoubleClick={() => open(row)}>
        <span className={row.item ? styles[row.item.severity] : undefined}><Icon name={row.item ? icons[row.item.severity] ?? 'info' : view.collapsed.includes(row.path) ? 'chevron-right' : 'chevron-down'} size={1} /></span>
        <span className={styles.message}>{row.label}</span>
        {row.item ? <><span className={styles.meta}>{row.item.source} {row.item.code}</span>{row.item.stale && <span className={styles.meta}>Changed since analysis</span>}<span className={styles.where}>{row.item.line}:{row.item.column}</span></> : <span className={styles.meta}>{row.count}</span>}
      </div>})}</div>}
    </div>
    {menu.menu}
  </section>
})
