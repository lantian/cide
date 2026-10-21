import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, type KeyboardEvent } from 'react'
import { defaultRangeExtractor, useVirtualizer, type Range } from '@tanstack/react-virtual'
import { PickerRow, PickerStatus } from '@/kit/components/Overlay'
import { Icon, type IconName } from '@/icons/Icon'
import { scaledRow, useUiScale } from '@/settings/useUiScale'
import { FileIcon } from '@/icons/FileIcon'
import { useIconTheme } from '@/icons/useIconTheme'
import type { IconTheme } from '@/icons/iconFor'
import { splitHighlight, trimIndent } from '@/sidebar/SearchModel'
import { SourcePreview } from './SourcePreview'
import { fileGroupOwners, location, resultLabel, resultRows, type Result, type ResultRow, type SearchMode } from './model'
import { EMPTY_VIEW, useSearchSessions } from './store'
import styles from './Search.module.css'

export interface ResultNavigation {
  move: (delta: number) => void;
  page: (direction: number) => void;
  expand: (expanded: boolean) => boolean;
  open: (modifier?: 'split' | 'mention' | 'session') => void;
}

export function ResultsView({ id, project, layout = 'sideBySide', items, mode, pinned = false, running, error, onOpen, onMode, selectedRef }: {
  id: string; project: string | null; layout?: 'sideBySide' | 'stacked'; items: Result[]; mode: SearchMode; pinned?: boolean; running: boolean; error?: string;
  onOpen: (result: Result, modifier?: 'split' | 'mention' | 'session') => void;
  onMode?: (mode: SearchMode) => void;
  selectedRef?: React.RefObject<ResultNavigation | null>;
}) {
  const state = useSearchSessions(s => s.views[id] ?? EMPTY_VIEW)
  const patch = (value: Partial<typeof state>) => useSearchSessions.getState().patchView(id, value)
  const rows = useMemo(() => resultRows(items, mode, state.collapsed, pinned), [items, mode, state.collapsed, pinned])
  const owners = useMemo(() => fileGroupOwners(rows), [rows])
  const active = rows.find(row => row.key === state.selected) ?? rows.find(row => row.kind !== 'group' && row.kind !== 'more') ?? rows[0] ?? null
  const selected = active && active.kind !== 'group' && active.kind !== 'more' ? active : null
  const scroll = useRef<HTMLDivElement>(null)
  const split = useRef<HTMLDivElement>(null)
  const pendingGroup = useRef<string | null>(null)
  const scale = useUiScale()
  const iconTheme = useIconTheme()
  const stacked = layout === 'stacked'
  const ratio = stacked ? state.stackedRatio : state.ratio
  const setRatio = (value: number) => patch({ [stacked ? 'stackedRatio' : 'ratio']: Math.max(25, Math.min(75, value)) })
  const height = (row: ResultRow) => scaledRow(row.kind === 'group' ? (row.level === 'provider' ? 40 : 36) : 30, scale)
  const rangeExtractor = useCallback((range: Range) => {
    const indexes = defaultRangeExtractor(range)
    const group = owners[range.startIndex]
    // Keep the canonical heading mounted even when its occurrences are far below it.
    if (group && !indexes.includes(group.header)) indexes.unshift(group.header)
    return indexes
  }, [owners])
  const virtual = useVirtualizer({ count: rows.length, getScrollElement: () => scroll.current, estimateSize: index => height(rows[index]!), getItemKey: index => rows[index]!.key,
    overscan: 10, rangeExtractor, scrollPaddingStart: owners.some(Boolean) ? scaledRow(36, scale) : 0 })
  const virtualItems = virtual.getVirtualItems()
  const stickyGroup = virtual.range ? owners[virtual.range.startIndex] : null
  const groupEnd = stickyGroup ? virtual.measurementsCache[stickyGroup.end]?.start ?? virtual.getTotalSize() : 0
  const revealIndex = (index: number) => {
    // Heights are fixed, so reveal in one step. scrollToIndex retries on later frames,
    // which would pull a subsequent wheel scroll or keyboard selection back to this row.
    const offset = virtual.getOffsetForIndex(index, 'auto')
    if (offset) virtual.scrollToOffset(offset[0])
  }
  useLayoutEffect(() => {
    virtual.measure()
    if (pendingGroup.current) {
      const index = rows.findIndex(row => row.key === pendingGroup.current)
      pendingGroup.current = null
      if (index >= 0) revealIndex(index)
    }
  }, [scale, rows, virtual])
  useEffect(() => { if (scroll.current) scroll.current.scrollTop = state.scroll }, [id])
  const move = (delta: number) => {
    const at = rows.findIndex(row => row.key === active?.key)
    const next = rows[Math.max(0, Math.min(rows.length - 1, at + delta))]
    if (!next) return
    patch({ selected: next.key })
    revealIndex(rows.findIndex(row => row.key === next.key))
  }
  const page = (direction: number) => {
    if (!scroll.current || rows.length === 0) return
    const at = Math.max(0, rows.findIndex(row => row.key === active?.key))
    const distance = Math.max(height(rows[at]!), scroll.current.clientHeight - height(rows[at]!))
    let next = at, moved = 0
    while (moved < distance && next + direction >= 0 && next + direction < rows.length) {
      moved += height(rows[next]!)
      next += direction
    }
    move(next - at)
  }
  const setExpanded = (row: Extract<ResultRow, { kind: 'group' }>, expanded: boolean) => {
    const collapsed = new Set(state.collapsed)
    if (expanded) collapsed.delete(row.key)
    else collapsed.add(row.key)
    pendingGroup.current = row.key
    patch({ collapsed, selected: row.key })
  }
  const expand = (expanded: boolean) => {
    if (active?.kind !== 'group') return false
    if (active.collapsed === expanded) setExpanded(active, expanded)
    return true
  }
  const activate = (modifier?: 'split' | 'mention' | 'session') => {
    if (active?.kind === 'group') setExpanded(active, active.collapsed)
    else if (active?.kind === 'more') onMode?.(active.mode)
    else if (selected) onOpen(selected, modifier)
  }
  if (selectedRef) selectedRef.current = { move, page, expand, open: activate }
  const keyDown = (e: KeyboardEvent) => {
    if (e.target !== e.currentTarget || e.nativeEvent.isComposing) return
    if ((e.key === 'ArrowLeft' || e.key === 'ArrowRight') && !e.shiftKey && !e.ctrlKey && !e.metaKey && !e.altKey
      && expand(e.key === 'ArrowRight')) e.preventDefault()
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') { e.preventDefault(); move(e.key === 'ArrowDown' ? 1 : -1) }
    if (e.key === 'PageDown' || e.key === 'PageUp') { e.preventDefault(); page(e.key === 'PageDown' ? 1 : -1) }
    if (e.key === 'Enter' && !e.ctrlKey && !e.metaKey) { e.preventDefault(); activate(e.shiftKey ? 'split' : e.altKey ? 'mention' : undefined) }
  }
  const click = (row: ResultRow) => {
    scroll.current?.focus({ preventScroll: true })
    if (row.kind === 'group') setExpanded(row, row.collapsed)
    else if (row.kind === 'more') onMode?.(row.mode)
    else patch({ selected: row.key })
  }
  return <div ref={split} className={styles.split} style={{ '--search-list-share': `${ratio}%` } as React.CSSProperties} data-audit="searchResults" data-layout={layout}>
    <div className={styles.resultsColumn}>
      {error && <div className={styles.status} role="status">{error}</div>}
      {items.length === 0 && <PickerStatus>{running ? 'Searching…' : 'No results. Try another query.'}</PickerStatus>}
      <div ref={scroll} className={styles.resultsScroll} role="listbox" aria-label="Search results" tabIndex={0}
        aria-activedescendant={active && virtualItems.some(item => rows[item.index]?.key === active.key) ? `search-result-${id}-${active.key}` : undefined}
        onKeyDown={keyDown} onScroll={e => patch({ scroll: e.currentTarget.scrollTop })}>
        <div style={{ height: virtual.getTotalSize(), position: 'relative' }}>
          {virtualItems.map(item => {
            const row = rows[item.index]
            if (!row) return null
            const isResult = row.kind !== 'group' && row.kind !== 'more'
            const sticky = stickyGroup?.header === item.index
            // Push this heading out as its last occurrence leaves the viewport.
            const push = sticky ? Math.min(0, groupEnd - (virtual.scrollOffset ?? 0) - item.size) : 0
            return <PickerRow key={row.key} virtual selected={active?.key === row.key}
              disabled={row.kind === 'action' && row.command.unavailable !== null}
              id={`search-result-${id}-${row.key}`} data-audit="searchResultRow" data-heading={row.kind === 'group' ? row.level : undefined}
              data-sticky={sticky || undefined}
              aria-expanded={row.kind === 'group' ? !row.collapsed : undefined}
              title={isResult ? resultLabel(row) : row.kind === 'group' ? row.label : 'Show all results'}
              style={sticky ? { height: item.size, position: 'sticky', top: 0, zIndex: 1, transform: `translateY(${push}px)` }
                : { height: item.size, transform: `translateY(${item.start}px)` }}
              onClick={() => click(row)} onDoubleClick={() => { if (isResult) onOpen(row) }}>
              {row.kind === 'group' ? <><Icon name={row.collapsed ? 'chevron-right' : 'chevron-down'} size={1} />{row.level === 'file' ? <FileLabel path={row.label} theme={iconTheme} /> : <strong className={styles.resultText}>{row.label}</strong>}<span className={styles.meta}>{row.count}</span></>
                : row.kind === 'more' ? <span className={styles.more}>Show all {row.count} {row.mode} results…</span>
                : <ResultContent result={row} theme={iconTheme} />}
            </PickerRow>
          })}
        </div>
      </div>
    </div>
    <div className={styles.divider} role="separator" tabIndex={0} aria-label="Resize results and preview" aria-orientation={stacked ? 'horizontal' : 'vertical'}
      aria-valuemin={25} aria-valuemax={75} aria-valuenow={ratio}
      onKeyDown={e => { const negative = stacked ? 'ArrowUp' : 'ArrowLeft'; const positive = stacked ? 'ArrowDown' : 'ArrowRight'; if (e.key === negative || e.key === positive) { e.preventDefault(); setRatio(ratio + (e.key === negative ? -5 : 5)) } }}
      onPointerDown={e => { e.currentTarget.setPointerCapture(e.pointerId); e.preventDefault() }}
      onPointerMove={e => { if (!e.currentTarget.hasPointerCapture(e.pointerId) || !split.current) return; const box = split.current.getBoundingClientRect(); setRatio((stacked ? (e.clientY - box.top) / box.height : (e.clientX - box.left) / box.width) * 100) }}
      onPointerUp={e => { if (e.currentTarget.hasPointerCapture(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId) }} />
    <SourcePreview result={selected} project={project} editorId={pinned ? id : undefined} />
  </div>
}
function FileLabel({ path, theme }: { path: string; theme: IconTheme }) {
  const slash = path.lastIndexOf('/')
  const name = path.slice(slash + 1)
  return <><FileIcon row={{ kind: 'file', name }} theme={theme} /><span className={styles.fileLabel}><strong>{name}</strong>{slash >= 0 && <span className={styles.fileDirectory}>{path.slice(0, slash)}</span>}</span></>
}
function ResultContent({ result, theme }: { result: Result; theme: IconTheme }) {
  const icons: Record<Result['kind'], IconName> = { file: 'file-text', symbol: 'code', text: 'search', action: 'chevron-right' }
  const at = location(result)
  if (result.kind === 'text') {
    const hit = trimIndent(result.hit)
    const parts = splitHighlight(hit.text, hit.start, hit.end)
    return <><span className={styles.lineNumber}>{result.hit.line}</span><span className={styles.code}>{parts.before}<mark>{parts.match}</mark>{parts.after}</span></>
  }
  if (result.kind === 'file') return <><FileLabel path={result.file.text} theme={theme} /><span className={styles.meta}>{result.file.source}</span></>
  return <><Icon name={icons[result.kind]} size={1} /><span className={styles.resultText}>
    {result.kind === 'symbol' ? result.symbol.name : result.command.title}
  </span><span className={styles.meta}>{result.kind === 'symbol' ? `${result.symbol.rel}:${at?.line}` : result.command.unavailable ?? result.shortcut ?? result.command.group}</span></>
}
