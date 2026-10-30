/**
 * What Local History's rows say, as pure functions. Import-free of anything with a runtime, so
 * `check:local-history` can compile this file alone and run it under Node.
 */
import type {
  LocalHistoryGroup,
  LocalVersion,
  VersionKind,
  VersionSource,
} from '@/ipc/generated'

const SOURCES: Record<VersionSource, string> = {
  saved: 'Saved',
  external: 'External change',
  detected: 'Detected',
  restored: 'Restored',
  fileOp: 'Moved',
}

const KINDS: Record<VersionKind, string> = {
  baseline: 'First seen',
  baselineDone: 'First seen',
  created: 'Created',
  modified: 'Changed',
  deleted: 'Deleted',
  label: 'Label',
}

export function sourceLabel(source: VersionSource): string {
  return SOURCES[source]
}

export function kindLabel(kind: VersionKind): string {
  return KINDS[kind]
}

/** A row's one line of what happened. A rename says where it went, not that it was deleted. */
export function versionTitle(version: LocalVersion): string {
  if (version.label !== null) return version.label
  if (version.moved !== null) {
    return version.kind === 'deleted' ? `Moved to ${version.moved}` : `Moved from ${version.moved}`
  }
  return kindLabel(version.kind)
}

export function groupTitle(group: LocalHistoryGroup): string {
  if (group.label !== null) return group.label
  if (group.snapshot) return `First snapshot, ${fileCount(group.files)}`
  return fileCount(group.files)
}

function fileCount(files: number): string {
  return files === 1 ? '1 file' : `${files.toLocaleString('en-US')} files`
}

/** Why a version cannot be restored, or `null` when it can. Spelled for a disabled control. */
export function restoreRefusal(version: LocalVersion): string | null {
  if (version.kept) return null
  return version.kind === 'deleted'
    ? 'This row records the deletion. Pick an earlier version to get the file back.'
    : 'This file was over the size limit, so no copy of this version was kept.'
}

/** Above this the editable compare draws nothing useful: `clipboardDiffView`'s own limit. */
const COMPARE_LIMIT = 1 << 20

/** Why a version cannot be opened for block-by-block restore, or `null` when it can. */
export function compareRefusal(version: LocalVersion): string | null {
  if (!version.kept) return restoreRefusal(version)
  return version.size > COMPARE_LIMIT
    ? 'This version is over 1 MiB, so it can only be restored whole.'
    : null
}

/** The one-line account of a folder restore. Every count named, none of them left to a list. */
export function reportLine(report: {
  written: number
  removed: number
  left: number
  failed: readonly unknown[]
}): string {
  const parts: string[] = []
  if (report.written > 0) parts.push(`${fileCount(report.written)} restored`)
  if (report.removed > 0) parts.push(`${fileCount(report.removed)} moved to the trash`)
  if (report.left > 0) parts.push(`${fileCount(report.left)} left as they are`)
  if (report.failed.length > 0) parts.push(`${fileCount(report.failed.length)} failed`)
  return parts.length === 0 ? 'Nothing needed restoring' : parts.join(', ')
}

function pad(value: number): string {
  return value < 10 ? `0${value}` : String(value)
}

export function timeLabel(ts: number): string {
  const at = new Date(ts)
  return `${pad(at.getHours())}:${pad(at.getMinutes())}:${pad(at.getSeconds())}`
}

/** The stable name of a local calendar day. */
function dayKey(at: Date): string {
  return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}`
}

export function dayLabel(ts: number, now: number): string {
  const key = dayKey(new Date(ts))
  if (key === dayKey(new Date(now))) return 'Today'
  const yesterday = new Date(now)
  yesterday.setDate(yesterday.getDate() - 1)
  if (key === dayKey(yesterday)) return 'Yesterday'
  return key
}

export type TimelineLine<T> =
  | { kind: 'day'; key: string; label: string }
  | { kind: 'row'; key: string; row: T }

/** Rows newest first, each day headed once. */
export function withDays<T extends { ts: number }>(
  rows: readonly T[],
  now: number,
  keyOf: (row: T) => string,
): TimelineLine<T>[] {
  const out: TimelineLine<T>[] = []
  let day: string | null = null
  for (const row of rows) {
    const next = dayKey(new Date(row.ts))
    if (next !== day) {
      day = next
      out.push({ kind: 'day', key: `day:${next}`, label: dayLabel(row.ts, now) })
    }
    out.push({ kind: 'row', key: keyOf(row), row })
  }
  return out
}

/**
 * The two sides of a diff, older first, whichever order the rows were clicked in.
 *
 * One selection compares with the file as it is now. Two compare with each other.
 */
export function comparedPair(
  selected: LocalVersion,
  other: LocalVersion | null,
): { from: LocalVersion; to: LocalVersion | null } {
  if (other === null) return { from: selected, to: null }
  return selected.id < other.id
    ? { from: selected, to: other }
    : { from: other, to: selected }
}
