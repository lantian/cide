import type { Command, PickerRow, SearchDescriptor, SearchHit, SearchSessionFrame, SymbolRow } from '../ipc/generated'
import { hitPosition } from '../sidebar/SearchModel'

export type SearchMode = SearchDescriptor['source'] | 'actions'
export const MODES: readonly SearchMode[] = ['all', 'files', 'symbols', 'actions', 'text']
export const MODE_LABELS: Record<SearchMode, string> = { all: 'All', files: 'Files', symbols: 'Symbols', actions: 'Actions', text: 'Text' }
export type Result =
  | { kind: 'file'; key: string; file: PickerRow }
  | { kind: 'symbol'; key: string; symbol: SymbolRow }
  | { kind: 'text'; key: string; hit: SearchHit }
  | { kind: 'action'; key: string; command: Command; shortcut: string | null }
export type ResultRow =
  | { kind: 'group'; level: 'provider' | 'file'; key: string; label: string; count: number; collapsed: boolean }
  | { kind: 'more'; key: string; mode: SearchMode; count: number }
  | Result

export function results(frame: SearchSessionFrame | null, commands: readonly Command[] = [], shortcutFor: (id: string) => string | null = () => null): Result[] {
  return [
    ...(frame?.files?.items ?? []).map((file): Result => ({ kind: 'file', key: `file:${file.value}`, file })),
    ...(frame?.symbols?.items ?? []).map((symbol): Result => ({ kind: 'symbol', key: `symbol:${symbol.path}:${symbol.line}:${symbol.column}:${symbol.name}`, symbol })),
    ...commands.map((command): Result => ({ kind: 'action', key: `action:${command.id}`, command, shortcut: shortcutFor(command.id) })),
    ...(frame?.text?.hits ?? []).map((hit, index): Result => ({ kind: 'text', key: `text:${hit.path}:${hit.line}:${hit.start}:${index}`, hit })),
  ]
}

export function location(result: Result): { path: string; line: number; column: number; endColumn: number } | null {
  switch (result.kind) {
    case 'file': return { path: result.file.value, line: 1, column: 1, endColumn: 1 }
    case 'symbol': return { path: result.symbol.path, line: result.symbol.line, column: result.symbol.column, endColumn: result.symbol.endColumn }
    case 'text': return { path: result.hit.path, ...hitPosition(result.hit) }
    case 'action': return null
  }
}
export function resultLabel(result: Result): string {
  switch (result.kind) {
    case 'file': return result.file.text
    case 'symbol': return `${result.symbol.name} — ${result.symbol.rel}:${result.symbol.line}`
    case 'text': return `${result.hit.rel}:${result.hit.line}  ${result.hit.text.trim()}`
    case 'action': return result.command.title
  }
}

/** Group keys stay stable as pages arrive. A saved selection names an occurrence, never a row index. */
export function resultRows(items: readonly Result[], mode: SearchMode, collapsed: ReadonlySet<string>, pinned: boolean): ResultRow[] {
  const rows: ResultRow[] = []
  const groups: [SearchMode, Result['kind']][] = [['files', 'file'], ['symbols', 'symbol'], ['actions', 'action'], ['text', 'text']]
  for (const [source, kind] of groups) {
    if (mode !== 'all' && mode !== source) continue
    const matches = items.filter(item => item.kind === kind)
    if (matches.length === 0) continue
    const key = `provider:${source}`
    if (mode === 'all') rows.push({ kind: 'group', level: 'provider', key, label: MODE_LABELS[source], count: matches.length, collapsed: collapsed.has(key) })
    if (collapsed.has(key)) continue
    const visible = mode === 'all' && !pinned ? matches.slice(0, 20) : matches
    const counts = new Map<string, number>()
    for (const item of matches) if (item.kind === 'text') counts.set(item.hit.path, (counts.get(item.hit.path) ?? 0) + 1)
    let lastPath = ''
    for (const item of visible) {
      if (item.kind === 'text') {
        const fileKey = `path:${item.hit.path}`
        if (lastPath !== item.hit.path) {
          rows.push({ kind: 'group', level: 'file', key: fileKey, label: item.hit.rel,
            count: counts.get(item.hit.path) ?? 0,
            collapsed: collapsed.has(fileKey) })
          lastPath = item.hit.path
        }
        if (collapsed.has(fileKey)) continue
      }
      rows.push(item)
    }
    if (visible.length < matches.length) rows.push({ kind: 'more', key: `more:${source}`, mode: source, count: matches.length })
  }
  return rows
}

/** Each file heading owns only its occurrences, ending before the next group or “more” row. */
export function fileGroupOwners(rows: readonly ResultRow[]): ({ header: number; end: number } | null)[] {
  let group: { header: number; end: number } | null = null
  return rows.map((row, index) => {
    if (row.kind !== 'text') {
      if (group) group.end = index
      group = row.kind === 'group' && row.level === 'file' ? { header: index, end: rows.length } : null
    }
    return group
  })
}

export function needsAnotherPage(frame: SearchSessionFrame, held: number): boolean {
  return frame.running || (frame.text !== null && held < frame.text.total)
}
export function tabLabel(descriptor: SearchDescriptor): string {
  return `${MODE_LABELS[descriptor.source]}: ${descriptor.query.pattern}`
}
