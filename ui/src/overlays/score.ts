/**
 * Command-palette ranking, ported from `cide_core::commands::score`.
 *
 * The palette filters locally rather than round-tripping to Rust. That is not a shortcut:
 * `app_get_bootstrap` already delivers the whole `Vec<Command>` — a few dozen entries — so
 * the table is in the window before the first keystroke, and an IPC hop per character would
 * put a round trip in the render path for a list that fits in a cache line's worth of work.
 * The file picker is the opposite case and stays in Rust, because its candidate set is a
 * repository.
 *
 * The port is line-for-line with the Rust so the two agree about ordering. They are held in
 * step by `ui/scripts/check-picker.mjs`, which asserts the tier boundaries the Rust tests
 * assert. If the Rust scorer changes, this must change with it — the alternative was to
 * expose `commands.search` as a command and pay the hop, and that loses on latency for a
 * list this size.
 *
 * DOM-free and import-free, so the check script can compile it on its own.
 */

/** The subset of the generated `Command` type this module needs. */
export interface Scorable {
  id: string
  title: string
}

/** Ranking tiers, best first. Same numbers as the Rust, so a diff between them is visible. */
const TIER_TITLE_PREFIX = 5
const TIER_TITLE_WORD = 4
const TIER_TITLE_SUBSTRING = 3
const TIER_ID = 2
const TIER_SUBSEQUENCE = 1

interface Score {
  tier: number
  /**
   * Where in the text the match landed. Earlier matches rank first within a tier, so
   * `Close pane` beats `Detach pane into window` for `pane`.
   */
  offset: number
}

/** Offset at which `needle` starts a word in `haystack`, or `-1`. */
function wordPrefix(haystack: string, needle: string): number {
  for (let i = 0; i < haystack.length; i++) {
    const prev = i === 0 ? null : (haystack[i - 1] as string)
    // Words break on anything non-alphanumeric, which covers the spaces and the colon in
    // titles like `Split: new Claude session` — typing `new` should find it.
    const atWordStart = prev === null || !/[a-z0-9]/i.test(prev)
    if (atWordStart && haystack.startsWith(needle, i)) return i
  }
  return -1
}

/** Do `needle`'s characters appear in `haystack` in order, gaps allowed? `spr` → `Split pane right`. */
function isSubsequence(haystack: string, needle: string): boolean {
  let at = 0
  for (const wanted of needle) {
    const found = haystack.indexOf(wanted, at)
    if (found < 0) return false
    at = found + 1
  }
  return true
}

/** `null` when the command does not match at all. */
function scoreOne(command: Scorable, needle: string): Score | null {
  const title = command.title.toLowerCase()

  if (title.startsWith(needle)) return { tier: TIER_TITLE_PREFIX, offset: 0 }

  const word = wordPrefix(title, needle)
  if (word >= 0) return { tier: TIER_TITLE_WORD, offset: word }

  const substring = title.indexOf(needle)
  if (substring >= 0) return { tier: TIER_TITLE_SUBSTRING, offset: substring }

  const inId = command.id.toLowerCase().indexOf(needle)
  if (inId >= 0) return { tier: TIER_ID, offset: inId }

  if (isSubsequence(title, needle)) return { tier: TIER_SUBSEQUENCE, offset: 0 }

  return null
}

/**
 * Commands matching `query`, best first.
 *
 * An empty or whitespace-only query returns the table unchanged, which is what the palette
 * shows before the user types.
 */
export function searchCommands<T extends Scorable>(table: readonly T[], query: string): T[] {
  const needle = query.trim().toLowerCase()
  if (needle === '') return [...table]

  const hits: { score: Score; index: number; command: T }[] = []
  table.forEach((command, index) => {
    const score = scoreOne(command, needle)
    if (score !== null) hits.push({ score, index, command })
  })

  // `Array.prototype.sort` is stable in every engine we ship to, but the tie-break on the
  // original index is written out rather than relied on: the property being asserted is
  // "equally scored commands keep table order", and stating it costs one comparison.
  hits.sort(
    (a, b) =>
      b.score.tier - a.score.tier || a.score.offset - b.score.offset || a.index - b.index,
  )
  return hits.map((hit) => hit.command)
}
