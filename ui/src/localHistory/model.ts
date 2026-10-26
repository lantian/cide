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
export function policyFromForm(enabled: boolean, days: string, budgetMiB: string, exclusions: string): { enabled: boolean; retentionDays: number; budgetBytes: number; exclusions: string[] } {
  const retentionDays = Number(days)
  const budgetBytes = Math.round(Number(budgetMiB) * 1024 * 1024)
  if (!Number.isSafeInteger(retentionDays) || retentionDays <= 0 || retentionDays > 0xffffffff) throw new Error('Retention must be a positive whole number of days.')
  if (!Number.isSafeInteger(budgetBytes) || budgetBytes <= 0 || budgetBytes > 0xffffffff) throw new Error('The budget must fit a positive byte count below 4096 MiB.')
  const patterns = exclusions.split('\n').map((s) => s.trim()).filter(Boolean)
  if (patterns.length > 256 || patterns.some((p) => p.length > 1024 || p.startsWith('/') || p.includes('\\') || p.includes('\0') || p.split('/').some((part) => part === '..' || part === '.'))) throw new Error('Use at most 256 root-relative glob exclusions without parent traversal.')
  return { enabled, retentionDays, budgetBytes, exclusions: patterns }
}
