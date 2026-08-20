/**
 * The branch control's list: what it offers, what typing narrows it to, and where ↑/↓ land.
 *
 * Import-free on purpose, like `logModel.ts` beside it and for the same reason —
 * `ui/scripts/check-log.mjs` compiles this file standalone and *executes* it. A filter rule that
 * lives inside a component is a rule no check can run, and this one has three edge cases that
 * are invisible until somebody hits them: an empty query is not "match everything", the two
 * synthetic rows are not branches, and the cap has to be applied after filtering rather than
 * before.
 *
 * # Why the control stopped being a `<select>`
 *
 * A native dropdown cannot be typed into. With one repository that is merely slightly slow; with
 * a few hundred branches — which is the ordinary state of a shared repository — picking one means
 * scrolling a list ordered by commit date looking for a name. Every other picker in this app is a
 * filter box, and this was the one place that made the user hunt.
 *
 * What is kept from the old control is the part that was right: `branchValue`/`branchFromValue`
 * still encode the choice as a string, so the two synthetic rows and a real branch name travel
 * through one channel and `LogFilter.branch` is unchanged.
 */

/**
 * How many branches the list shows at once.
 *
 * Fifty, and it is a *display* cap rather than a fetch cap: `branch.list` returns everything and
 * the names arrive already ordered most-recently-committed-first, so the first fifty are the
 * fifty a person is plausibly looking for. Typing narrows the whole set, not the visible fifty —
 * that ordering is exactly why the cap is safe, and it is also why filtering must happen before
 * the slice. Slicing first would make a branch impossible to reach by typing its name, which is
 * the one thing the box exists to do.
 */
export const BRANCH_LIST_MAX = 50

/** A row of the dropdown. The two synthetic ones are not branches and are never filtered out. */
export interface BranchRow {
  /** The `LogFilter.branch` encoding — `head`, `all`, `b:<name>`, or `r:<spec>`. */
  readonly value: string
  readonly label: string
  /** `true` for *Current branch* and *All branches*, which are readings rather than refs. */
  readonly synthetic: boolean
}

/**
 * Case-insensitive substring, which is what every other filter in this app means by "matches".
 *
 * Not fuzzy. `chrome/switcher` and the file picker are fuzzy because they search a space the user
 * half-remembers; a branch name is something the user *knows*, and fuzzy matching on known input
 * mostly produces surprising near-misses above the exact one. `feat` should not rank
 * `f-e-a-t-u-r-e-s` above `feature/login`.
 */
function matches(name: string, query: string): boolean {
  return name.toLowerCase().includes(query.toLowerCase())
}

/**
 * The rows to draw for a query.
 *
 * `query` empty — the focused-but-untyped state — is **the full list**, capped. That is the
 * behaviour asked for and it is worth stating because the obvious implementation of "filter"
 * returns nothing for an empty needle in some libraries and everything in others.
 *
 * `current` is the chosen value, and it is here for one case: a walk re-rooted at a bare
 * revision, which *Clear filters and find it* sets. That value names no branch, so nothing in
 * the list would match it and the box would look empty while holding a real filter. It is
 * appended rather than sorted in, because it is not something the user can pick — it is a
 * reading of what is already chosen.
 */
export function branchRows(
  all: readonly string[],
  query: string,
  current: string,
  currentLabel: string,
): BranchRow[] {
  const q = query.trim()
  const rows: BranchRow[] = []

  // HEAD first because it is the default and the answer most people want; *All branches* is
  // `--all`, the one other whole-repository reading. Named rather than spelled as git flags:
  // nobody types `--all` into IDEA either, and the flag is the implementation.
  const synthetic: BranchRow[] = [
    { value: 'head', label: 'Current branch', synthetic: true },
    { value: 'all', label: 'All branches', synthetic: true },
  ]
  for (const row of synthetic) {
    if (q === '' || matches(row.label, q)) rows.push(row)
  }

  if (current.startsWith('r:')) {
    rows.push({ value: current, label: currentLabel, synthetic: true })
  }

  // Filter first, cap second. The other order makes a branch unreachable by typing its name.
  let shown = 0
  for (const name of all) {
    if (q !== '' && !matches(name, q)) continue
    if (shown >= BRANCH_LIST_MAX) break
    rows.push({ value: `b:${name}`, label: name, synthetic: false })
    shown += 1
  }
  return rows
}

/**
 * How many branches the query matched in total, which is not `rows.length`.
 *
 * The footnote under a capped list has to say how many were left out, and it cannot count the
 * rows — those include the synthetic entries and stop at the cap by construction. A list that
 * silently shows fifty of four hundred is the failure `LogStop::Budget` exists to prevent one
 * surface over.
 */
export function branchMatchCount(all: readonly string[], query: string): number {
  const q = query.trim()
  if (q === '') return all.length
  let n = 0
  for (const name of all) if (matches(name, q)) n += 1
  return n
}

/** `null` when nothing is hidden, else the sentence saying so. */
export function branchOverflowNote(all: readonly string[], query: string): string | null {
  const total = branchMatchCount(all, query)
  if (total <= BRANCH_LIST_MAX) return null
  return `${total - BRANCH_LIST_MAX} more — keep typing to narrow`
}

/**
 * Where ↑/↓/Home/End move the highlight.
 *
 * Clamped rather than wrapping. A wrapping list makes "hold ↓ to reach the bottom" impossible to
 * do without overshooting past the top, and every other list in this app — the palette, the file
 * picker — clamps for that reason.
 *
 * `-1` means nothing is highlighted, which is the state the box opens in: pressing ↓ then goes to
 * the first row rather than the second, and pressing Enter with nothing highlighted commits the
 * typed text rather than silently choosing row one.
 */
export function moveHighlight(count: number, current: number, key: string): number {
  if (count === 0) return -1
  switch (key) {
    case 'ArrowDown':
      return Math.min(current + 1, count - 1)
    case 'ArrowUp':
      return Math.max(current - 1, -1)
    case 'Home':
      return 0
    case 'End':
      return count - 1
    default:
      return current
  }
}

/**
 * What Enter does: the highlighted row, or the single remaining match, or nothing.
 *
 * The middle case is the one that makes the control feel finished. Typing a name until exactly
 * one branch is left and pressing Enter should choose it — requiring ↓ first to highlight the
 * only row on screen is a keystroke that carries no information.
 *
 * With several matches and no highlight it returns `null`: guessing the first would mean Enter
 * picks a branch the user never looked at, and the list is right there to arrow into.
 */
export function commitChoice(rows: readonly BranchRow[], highlight: number): BranchRow | null {
  if (highlight >= 0 && highlight < rows.length) return rows[highlight] ?? null
  const real = rows.filter((r) => !r.synthetic)
  return real.length === 1 ? (real[0] ?? null) : null
}
