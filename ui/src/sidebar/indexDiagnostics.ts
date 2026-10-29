import type { FsIndexDiagnostics, IndexAllocation, IndexBudgetUsage, IndexLimit } from '@/ipc/generated'

export const allocationLabels: Record<IndexAllocation, string> = {
  files: 'File index', libraries: 'Library index', symbols: 'Symbol index',
  watcher: 'Filesystem watcher', overhead: 'Index infrastructure',
}

export function indexLimitLabel(limit: IndexLimit): string {
  return limit === 'projectBudget' ? 'Project memory limit reached' : 'Combined memory limit reached'
}

export function indexMiB(bytes: number): string {
  return `${(bytes / (1024 * 1024)).toFixed(2)} MiB`
}

export function budgetLines(usage: IndexBudgetUsage): string[] {
  return [
    `Project: ${indexMiB(usage.projectBytes)} / ${indexMiB(usage.projectLimitBytes)}`,
    `Combined: ${indexMiB(usage.totalBytes)} / ${indexMiB(usage.totalLimitBytes)}`,
  ]
}

export const folderExplanation = 'Estimates cover currently indexed file-tree entries. Parent totals include descendants; rows overlap and must not be added together.'
export const budgetExplanation = 'Estimated index allocations, not disk sizes or total app memory. Overall usage also includes symbols, libraries, watcher allocations and retained capacity.'

export function formatIndexDiagnostics(report: FsIndexDiagnostics): string {
  const stop = report.refusal
  return [
    'Indexing diagnostics',
    `Generation: ${report.generation}`,
    `Indexing: ${report.indexing ? 'in progress' : 'stopped'}`,
    `Roots: ${report.roots.join(', ')}`,
    `Indexed: ${report.files} files, ${report.dirs} folders`,
    `Show hidden files: ${report.showHiddenFiles}; Show ignored files: ${report.showIgnoredFiles}`,
    `Indexing exclusions (${report.exclusionsSource}): ${report.indexExcludedFolders?.join(', ') || 'None'}`,
    `Explorer browsing cache: ${indexMiB(report.browseCacheBytes ?? 0)} / 16 MiB`,
    budgetExplanation,
    ...budgetLines(report.usage),
    ...(stop ? [
      indexLimitLabel(stop.reason),
      `Allocation: ${allocationLabels[stop.allocation]}`,
      `Path: ${stop.path ?? 'No path (infrastructure allocation)'}`,
      ...(stop.folder ? [`Containing folder: ${stop.folder}`] : []),
      `Requested: ${stop.requestedBytes} bytes (${indexMiB(stop.requestedBytes)})`,
      'Usage at first refusal:', ...budgetLines(stop.usage),
      'This is the stop point, not necessarily the largest contributor. Coverage is partial; unvisited folders cannot be ranked.',
    ] : ['No memory limit recorded for this generation.']),
    'Largest indexed folders (up to 10):', folderExplanation,
    ...report.folders.map((row) => `${row.path}\t${row.bytes} bytes (${indexMiB(row.bytes)})\t${row.files} files\t${row.dirs} folders`),
  ].join('\n')
}
