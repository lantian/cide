import type {
  Approval,
  Change,
  Version,
  Refs,
  Position,
  Discussion,
} from './types'
/** Repository-relative globs; a recursive directory wildcard also matches the root. */
export function globRegex(pattern: string): RegExp {
  pattern = pattern.trim().replace(/^\.\//, '')
  let value = pattern.includes('/') ? '^' : '^(?:.*/)?'
  for (let i = 0; i < pattern.length; i++) {
    const c = pattern[i]!
    if (c === '*' && pattern[i + 1] === '*') {
      i++
      if (pattern[i + 1] === '/') {
        i++
        value += '(?:.*/)?'
      } else value += '.*'
    } else if (c === '*') value += '[^/]*'
    else if (c === '?') value += '[^/]'
    else value += c.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  }
  return new RegExp(value + '$')
}
export function visibleChanges(
  changes: readonly Change[],
  patterns: readonly string[],
  enabled: boolean,
) {
  const visible = pathVisibility(patterns, enabled)
  return changes.filter((c) => visible(c.new_path || c.old_path))
}
export function changeTotals(changes: readonly Change[]) {
  let additions = 0,
    deletions = 0
  let complete = true
  for (const c of changes) {
    if (c.too_large || c.collapsed || (!c.diff && !c.renamed_file))
      complete = false
    let inHunk = false
    for (const line of c.diff.split('\n')) {
      if (line.startsWith('@@ ')) {
        inHunk = true
        continue
      }
      if (!inHunk) continue
      if (line.startsWith('+')) additions++
      if (line.startsWith('-')) deletions++
    }
  }
  return { files: changes.length, additions, deletions, complete }
}
export function versionRefs(v: Version): Refs {
  return {
    base_sha: v.base_commit_sha,
    start_sha: v.start_commit_sha,
    head_sha: v.head_commit_sha,
  }
}
export function linePosition(
  change: Change,
  refs: Refs,
  side: 'old' | 'new',
  number: number,
): Position | null {
  let old = 0,
    next = 0
  for (const text of change.diff.split('\n')) {
    const hunk = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(text)
    if (hunk) {
      old = Number(hunk[1])
      next = Number(hunk[2])
      continue
    }
    if (!old && !next) continue
    const origin = text[0]
    if (![' ', '+', '-'].includes(origin ?? '')) continue
    const oldLine = origin !== '+' ? old++ : undefined
    const newLine = origin !== '-' ? next++ : undefined
    if ((side === 'old' ? oldLine : newLine) === number)
      return {
        ...refs,
        position_type: 'text',
        old_path: change.old_path,
        new_path: change.new_path,
        ...(oldLine === undefined ? {} : { old_line: oldLine }),
        ...(newLine === undefined ? {} : { new_line: newLine }),
      }
  }
  return null
}
export function reviewProject(id: string, sha: string): string {
  const s = id.slice(0, 16) + sha.slice(0, 16)
  return `${s.slice(0, 8)}-${s.slice(8, 12)}-${s.slice(12, 16)}-${s.slice(16, 20)}-${s.slice(20)}`
}
export function message(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

/** GitLab's approvals endpoint can omit `approved`; approvals_left is authoritative. */
export function approvalStatus(approval: Approval | null, stale = false) {
  if (stale)
    return {
      kind: 'unknown',
      title: 'Refresh approval status',
      detail: 'New commits are available.',
    } as const
  if (!approval)
    return {
      kind: 'unknown',
      title: 'Approval status unavailable',
      detail: 'Refresh to check GitLab.',
    } as const
  if ((approval.approvals_left ?? 0) > 0) {
    const remaining = approval.approvals_left!
    return {
      kind: 'pending',
      title: 'Needs approval',
      detail: `${remaining} ${remaining === 1 ? 'approval' : 'approvals'} remaining`,
    } as const
  }
  const count = approval.approved_by.length
  if (approval.approvals_required === 0 && count === 0) {
    return {
      kind: 'optional',
      title: 'No approvals required',
      detail: 'You can still leave an approval.',
    } as const
  }
  if (approval.approved === true || approval.approvals_left === 0) {
    return {
      kind: 'approved',
      title: 'Approved',
      detail: count
        ? `${count} ${count === 1 ? 'person has' : 'people have'} approved`
        : 'All approval requirements met',
    } as const
  }
  if (approval.approved === false)
    return {
      kind: 'pending',
      title: 'Needs approval',
      detail: 'Approval requirements are not yet met.',
    } as const
  return {
    kind: 'unknown',
    title: 'Approval status unavailable',
    detail: 'GitLab did not report approval requirements.',
  } as const
}

/** The changed tree, full source tree and all counters use the same predicate. */
export function pathVisibility(patterns: readonly string[], enabled: boolean) {
  const rules = patterns.filter((p) => p.trim()).map(globRegex)
  return (path: string) => !enabled || !rules.some((rule) => rule.test(path))
}

export function threadResolved(thread: Discussion): boolean {
  const notes = thread.notes.filter((note) => note.resolvable)
  return notes.length > 0 && notes.every((note) => note.resolved)
}
/** Counts discussions, not individual replies; renames retain either-side anchors. */
export function fileDiscussions(
  threads: readonly Discussion[],
  path: string,
  oldPath = path,
) {
  let resolved = 0,
    unresolved = 0
  for (const thread of threads) {
    if (
      !thread.notes.some(
        (note) =>
          note.position &&
          [note.position.new_path, note.position.old_path].some(
            (p) => p === path || p === oldPath,
          ),
      )
    )
      continue
    if (threadResolved(thread)) resolved++
    else unresolved++
  }
  return { resolved, unresolved }
}

/**
 * The repository an MR belongs to, from its `web_url`: `host` and the project path before
 * `/-/merge_requests/`. `null` for anything else — the caller then falls back to the global
 * instructions rather than guessing a repository from half a URL.
 */
export function repositoryOfMr(
  webUrl: string | undefined,
): { host: string; path: string } | null {
  if (!webUrl) return null
  const match = /^[a-z]+:\/\/([^/]+)\/(.+?)\/-\/merge_requests\b/i.exec(webUrl)
  return match ? { host: match[1]!, path: match[2]! } : null
}

/**
 * What "Review with an agent"'s instructions box starts with. A repository entry names the MR's
 * repository as `group/repo`, `host/group/repo`, or — for a GitLab served under a subpath, whose
 * project path carries that subpath — any whole-segment tail of it with a `/` in it; the first
 * entry with a non-blank prompt wins, then the global prompt. A blank entry falls through rather
 * than meaning "no instructions here": the settings screen has no way to show the difference.
 * The entries are already normalised by `cide_gitlab::normalise_review_prompts` when saved; the
 * lower-casing here is because GitLab paths are case-insensitive and users type them freely.
 */
export function defaultReviewPrompt(
  preferences: {
    reviewPrompt: string
    reviewPrompts: readonly { repository: string; prompt: string }[]
  },
  webUrl: string | undefined,
): string {
  const repo = repositoryOfMr(webUrl)
  if (repo) {
    const path = repo.path.toLowerCase()
    const full = `${repo.host.toLowerCase()}/${path}`
    for (const entry of preferences.reviewPrompts) {
      const key = entry.repository.trim().replace(/^\/+|\/+$/g, '').toLowerCase()
      if (!key || !entry.prompt.trim()) continue
      if (
        key === path ||
        key === full ||
        (key.includes('/') && `/${path}`.endsWith(`/${key}`))
      )
        return entry.prompt
    }
  }
  return preferences.reviewPrompt.trim() ? preferences.reviewPrompt : ''
}
