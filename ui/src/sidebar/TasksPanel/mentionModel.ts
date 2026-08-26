/**
 * The pure half of @mention autocomplete: the token under the caret, the filtered options, and
 * the splice that applies a choice.
 *
 * # Import-free, like `model.ts`, and for the same stated reason
 *
 * `ui/scripts/check-agents.mjs` compiles this file **standalone** with a bare `tsc` — no
 * tsconfig, no path aliases — so an import of anything but the standard library breaks the
 * check outright, which is the tripwire. That is why the scorer below is a private ~20-line
 * restatement of `overlays/score.ts`'s tier idea rather than an import of it: the real thing
 * ranks `Scorable`s for the palette and carries subsequence matching this list does not want
 * (an id popup that matches `dvp` → `developer` offers spooky results while typing at speed).
 *
 * # What a mention is, and why the rules mirror Rust's
 *
 * The authoritative grammar lives in `cide-agents/src/mentions.rs` — the backend is what acts
 * on a committed comment — and this module's `mentionQuery` deliberately opens in the same
 * places that parser matches: an `@` at the start of the text or after a non-alphanumeric
 * character (`(@dev` opens; `user@example.com` does not, the `r` closes it). A popup that
 * opened where the parser would not act, or stayed shut where it would, teaches the user a
 * grammar the system does not have. The *insert* is the id, not the label: ids are the stable
 * vocabulary the parser, the roster and the chips all resolve.
 */

/** The @-token under the caret: where it starts (at the `@`), ends (the caret), and says. */
export interface MentionQuery {
  /** Index of the `@` itself. */
  start: number
  /** The caret — the splice in [`applyMention`] replaces `[start, end)`. */
  end: number
  /** What has been typed after the `@`, possibly empty. */
  query: string
}

/**
 * A query longer than any legal role name (Rust caps ids at 32) plus slack for typos. Without
 * a cap, a stray `@` at the top of three later paragraphs re-opens the popup every time the
 * caret wanders back into them.
 */
const QUERY_CAP = 40

/** Characters that extend the token being typed. Wider than the id alphabet on purpose — an
 * uppercase or underscore mid-token keeps the popup open (filtering to nothing) rather than
 * splitting one word into a fresh, wrong query. */
const TOKEN = /[A-Za-z0-9_-]/

/**
 * The @-token the caret is inside, or `null` when the popup has no business being open.
 *
 * Scans back from the caret over token characters to find the `@`; whitespace ends the scan
 * (a space closes the popup), and the character before the `@` must be non-alphanumeric or
 * the start of the text — the parser's own opening rule, argued in the header.
 */
export function mentionQuery(text: string, caret: number): MentionQuery | null {
  if (caret < 1 || caret > text.length) return null
  let start = caret
  while (start > 0 && TOKEN.test(text[start - 1] ?? '')) {
    start -= 1
    if (caret - start > QUERY_CAP) return null
  }
  if (start === 0 || text[start - 1] !== '@') return null
  const at = start - 1
  const before = at === 0 ? null : (text[at - 1] ?? null)
  if (before !== null && /[a-zA-Z0-9]/.test(before)) return null
  return { start: at, end: caret, query: text.slice(start, caret) }
}

/** One row of the popup. */
export interface MentionOption {
  id: string
  label: string
}

/**
 * The roster entries matching `query`, best first.
 *
 * Tiers, in order: id prefix, label word-prefix, id substring, label substring; no match is no
 * row. The empty query offers the whole roster. Within a tier the order is by id — stable and
 * predictable, which for a four-row popup beats any cleverness. Case-insensitive on the query
 * side only: ids are lowercase by Rust's rule, labels are whatever the author wrote.
 */
export function mentionOptions(
  roles: Readonly<Record<string, string>>,
  query: string,
): MentionOption[] {
  const needle = query.toLowerCase()
  const ranked: Array<{ tier: number; option: MentionOption }> = []
  for (const id of Object.keys(roles).sort()) {
    if (id.trim() === '') continue
    const label = roles[id] ?? id
    const tier = tierOf(id, label, needle)
    if (tier === null) continue
    ranked.push({ tier, option: { id, label } })
  }
  ranked.sort((a, b) => a.tier - b.tier || a.option.id.localeCompare(b.option.id))
  return ranked.map((entry) => entry.option)
}

function tierOf(id: string, label: string, needle: string): number | null {
  if (needle === '') return 0
  const lowerLabel = label.toLowerCase()
  if (id.startsWith(needle)) return 0
  if (lowerLabel.split(/\s+/).some((word) => word.startsWith(needle))) return 1
  if (id.includes(needle)) return 2
  if (lowerLabel.includes(needle)) return 3
  return null
}

/**
 * Splice the chosen id in over the token: `@<id> `, with the caret after the space.
 *
 * The trailing space is deliberate — it is what closes the popup ([`mentionQuery`] refuses a
 * token ending at whitespace) and it leaves the caret ready for prose, which is what everyone
 * types next.
 */
export function applyMention(
  text: string,
  at: MentionQuery,
  id: string,
): { text: string; caret: number } {
  const next = `${text.slice(0, at.start)}@${id} ${text.slice(at.end)}`
  return { text: next, caret: at.start + id.length + 2 }
}
