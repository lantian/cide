/**
 * *Next comment* / *Previous comment* in a GitLab review diff (Alt+Shift+Down/Up).
 *
 * `keys/dispatch.ts` knows which tab is focused — its `origin.document` — but has no DOM node and
 * no React state, so the mounted `ReviewDiff` for that document registers a walker here and the
 * dispatcher looks it up by the document. Keyed by document rather than stacked like
 * `panes/changeNav.ts`: dispatch already resolved *which* diff is in front (the focused tab), so
 * a stack would only be a second, possibly disagreeing, answer to a question already settled.
 * Two panes showing the same MR file share a key; the later mount wins and the earlier one's
 * release is a no-op because it removes only its own walker.
 *
 * The walk **stays in the file and clamps** at both ends (the user's call): Alt+PgDn already
 * moves between files, and a comment walk that silently crossed into another file would move the
 * tab under the reader. The clamp is `changeNav.stepIndex`'s rule, restated here so this module
 * stays import-free for a check to compile standalone.
 */

/** One stop: a line that carries at least one thread or agent draft. */
export interface CommentStop {
  readonly side: 'old' | 'new'
  readonly line: number
}

export interface CommentNav {
  /** Whether it moved. `false` at either end or with nothing to walk, for `unmet(...)`. */
  step(delta: 1 | -1): boolean
}

/**
 * Where a walk is, spelled `"index/count"` (index `-1` before the first step). A **string** so
 * `useSyncExternalStore` compares it with `Object.is` and the header's "2 / 5" does not loop —
 * the failure `changeNav.changeNavPresent` documents for an object snapshot.
 */
const cursors = new Map<string, string>()
const listeners = new Set<() => void>()

export function subscribeCommentNav(listen: () => void): () => void {
  listeners.add(listen)
  return () => {
    listeners.delete(listen)
  }
}

export function commentCursor(key: CommentNavKey): string {
  return cursors.get(keyOf(key)) ?? '-1/0'
}

/** Called by the walker's owner whenever its index or its number of stops changes. */
export function publishCommentCursor(key: CommentNavKey, index: number, count: number): void {
  const k = keyOf(key)
  const next = `${index}/${count}`
  if (cursors.get(k) === next) return
  cursors.set(k, next)
  for (const listen of [...listeners]) listen()
}

/** The fields that identify one review diff, in either spelling (`GitLabDocument` or ours). */
export interface CommentNavKey {
  readonly review: string
  readonly path: string
  readonly baseSha: string
  readonly headSha: string
}

const walkers = new Map<string, CommentNav>()

function keyOf(key: CommentNavKey): string {
  return [key.review, key.path, key.baseSha, key.headSha].join('\u0000')
}

/** Register the walker of a mounted review diff; the return releases exactly this one. */
export function claimCommentNav(key: CommentNavKey, nav: CommentNav): () => void {
  const k = keyOf(key)
  walkers.set(k, nav)
  return () => {
    if (walkers.get(k) === nav) walkers.delete(k)
  }
}

export function commentNavFor(key: CommentNavKey): CommentNav | null {
  return walkers.get(keyOf(key)) ?? null
}

/**
 * Stops in reading order: by line, and on one line the old side before the new. An old-side
 * line and a new-side line are different numbering schemes, but in a diff both run top to
 * bottom through the same hunks, and a mixed order is only ever off within a hunk — a far
 * smaller surprise than walking every old-side comment before any new-side one.
 */
export function orderStops(anchors: Iterable<string>): CommentStop[] {
  const stops: CommentStop[] = []
  for (const anchor of anchors) {
    const [side, n] = anchor.split(':')
    const line = Number(n)
    if ((side === 'old' || side === 'new') && Number.isFinite(line)) stops.push({ side, line })
  }
  return stops.sort((a, b) => a.line - b.line || (a.side === b.side ? 0 : a.side === 'old' ? -1 : 1))
}

/** `changeNav.stepIndex`: clamps, never wraps; nothing walked yet lands on the first stop. */
export function stepStop(count: number, current: number, delta: 1 | -1): number | null {
  if (count <= 0) return null
  if (current < 0) return 0
  const next = current + delta
  return next < 0 || next >= count ? null : next
}
