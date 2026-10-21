import { countBySeverity, type Diagnostic, type DiagnosticsSnapshot, type Severity } from '../sidebar/ProblemsPanel/model'

export interface ProblemsView {
  scope: 'project' | 'file'
  sort: 'position' | 'severity'
  severities: Record<Severity, boolean>
  source: string
  collapsed: readonly string[]
}
export const DEFAULT_PROBLEMS_VIEW: ProblemsView = {
  scope: 'project', sort: 'position',
  severities: { error: true, warning: true, info: true, hint: true },
  source: '', collapsed: [],
}
export function problemKey(item: Diagnostic): string {
  return JSON.stringify([item.absPath ?? item.path, item.line, item.column, item.endLine, item.endColumn, item.source, item.code, item.message])
}
export function problemDescription(item: Diagnostic): string {
  return `${item.path}:${item.line}:${item.column}: ${item.severity}: ${item.message}${item.code ? ` [${item.code}]` : ''}${item.source ? ` (${item.source})` : ''}`
}
export function problemGroups(snapshot: DiagnosticsSnapshot, view: ProblemsView, file: string | null) {
  const items = snapshot.kind === 'unavailable' ? [] : snapshot.items ?? []
  const rank = { error: 0, warning: 1, info: 2, hint: 3 }
  const groups = new Map<string, Diagnostic[]>()
  for (const item of items) {
    if (view.scope === 'file' && (file === null || (item.absPath ?? item.path) !== file)) continue
    if (view.severities[item.severity] === false || (view.source && item.source !== view.source)) continue
    const path = item.absPath ?? item.path
    const group = groups.get(path) ?? []
    group.push(item)
    groups.set(path, group)
  }
  return [...groups].sort(([a], [b]) => a.localeCompare(b)).map(([path, rows]) => ({
    path, label: rows[0]!.path,
    items: [...rows].sort((a, b) => (view.sort === 'severity' ? (rank[a.severity] ?? 4) - (rank[b.severity] ?? 4) : 0)
      || a.line - b.line || a.column - b.column || a.message.localeCompare(b.message)),
  }))
}
export function diagnosticIndicator(snapshot: DiagnosticsSnapshot) {
  const items = snapshot.kind === 'unavailable' ? [] : snapshot.items ?? []
  const counts = countBySeverity(items)
  const scanning = snapshot.kind === 'scanning' || snapshot.sources?.some(s => s.status.kind === 'scanning') === true
  const partial = snapshot.kind === 'unavailable' || snapshot.sources?.some(s => s.status.kind === 'unavailable' && !s.status.reason.startsWith('Turned off in Settings')) === true
  const clean = snapshot.kind === 'ready' && !scanning && !partial && items.length === 0 && !snapshot.truncated
  return { counts, scanning, partial, clean }
}
