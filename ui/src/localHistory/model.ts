export class HistoryRequests {
  private version = 0
  begin(): number { return ++this.version }
  cancel(): void { ++this.version }
  accepts(request: number): boolean { return request === this.version }
}
export function appendUnique<T>(before: T[], after: T[], key: (item: T) => string): T[] {
  const seen = new Set(before.map(key))
  return [...before, ...after.filter((item) => { const id = key(item); if (seen.has(id)) return false; seen.add(id); return true })]
}
export const HISTORY_SOURCES: Record<string, string> = { baseline: 'Baseline', reopen: 'Reopen observation', cideSave: 'cide save', external: 'External observation', restore: 'Restore', fileOperation: 'cide file operation' }
export function historyTime(ms: number): string { return new Date(ms).toLocaleString() }
export interface HistoryPage<T, Cursor> { items: T[]; next: Cursor | null; loading: boolean; error: string | null; excluded?: string | null }
export function comparisonMatches(comparison: { token: { generation: number; path: string; target: string }; right: { revision: string | null } } | null, path: string | null, target: string | null, other: string | null, generation: number): boolean {
  return comparison !== null && comparison.token.generation === generation && comparison.token.path === path && comparison.token.target === target && comparison.right.revision === other
}
