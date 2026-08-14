/**
 * The find bar's number: how many matches there are, and which one you are standing on.
 *
 * # Why there is an ordinal at all
 *
 * `@codemirror/search` wraps, silently, in both directions and for both query kinds —
 * `StringQuery.nextMatch` restarts from `0` when its cursor runs out, `prevMatch` restarts from
 * the far end, and `RegExpQuery` does the same. Nothing says so. The bar used to render a bare
 * total (`12 matches`), so passing the last match and landing back on the first was
 * indistinguishable from the key having done nothing at all: the number did not move, and on a
 * screen where every match is highlighted the selection moving off-screen is not obvious either.
 * The only wrap signal anywhere in the stack is `announceMatch`, an aria-live announcement —
 * screen readers were told and sighted users were not.
 *
 * `3 of 12` is the fix, and it is the fix precisely because `12 → 1` is unmistakable. A
 * transient "wrapped" notice was the alternative and it is worse twice over: it says nothing
 * when the user is not looking at that pixel, and it answers a question nobody asked instead of
 * the one they did, which is *where am I in this file*. It is also what VS Code and IDEA show.
 *
 * # Why this is a module and not four lines in the panel
 *
 * `find.ts` needs a live `EditorView`, a `SearchQuery` and a mounted DOM to exist, so
 * `ui/scripts/check-editor.mjs` cannot compile it — the script's own header says so. Everything
 * here is arithmetic over plain numbers and it is the half that can be got wrong quietly: an
 * off-by-one in the ordinal, an ordinal that survives past the cap, a wrap that reports `0`.
 * This module imports nothing for exactly that reason (the rule `caretTrack.ts`,
 * `lintMap.ts` and `highlightLevel.ts` already follow), so the check compiles it standalone with
 * a bare `tsc` and runs it under node.
 */

/**
 * Past this many matches the bar stops counting and says so.
 *
 * Counting means walking the document: `SearchCursor` has no budget, so a one-character query on
 * a 5 MB file matches often enough that an exact figure costs more than it tells anyone. Past the
 * cap the panel says `999+` and the walk stops — which is also why the ordinal is suppressed
 * there: the walk may have stopped *before* reaching the selected match, so the honest answer to
 * "which one is this" past the cap is silence rather than a number that is sometimes right.
 */
export const COUNT_CAP = 999

/** A match, in document positions. Half-open, as CodeMirror's cursor reports it. */
export interface MatchSpan {
  readonly from: number
  readonly to: number
}

/** What the counter draws, before it is words. */
export interface Tally {
  /** Matches found. Never above [`COUNT_CAP`] + 1 — the walk stops there. */
  readonly total: number
  /** The walk hit the cap and stopped, so `total` is a floor and not a figure. */
  readonly capped: boolean
  /** 1-based position of the selected match, or `null` when the selection is not on one. */
  readonly ordinal: number | null
}

/**
 * Walk the matches, counting them and looking for the one the selection is sitting on.
 *
 * One walk, not two. The total and the ordinal are the same scan, and doing them separately
 * would double the cost of the most expensive thing the find bar does — on the file size the cap
 * above exists for, that is the difference between a counter that lags a frame and one that
 * stutters the buffer.
 *
 * `matches` is an `Iterable` rather than a cursor so this module can stay free of
 * `@codemirror/search`; `find.ts` adapts the real cursor in three lines. It is consumed lazily
 * and abandoned at the cap, so a generator over a five-megabyte document does not run to
 * completion.
 *
 * The selection is matched on *both* ends. `from` alone would claim an ordinal for a caret
 * parked at the start of a match after the user clicked there, which reads as "you are on match
 * 3" when nothing has been found — the number would appear before the search did anything.
 */
export function tally(matches: Iterable<MatchSpan>, selection: MatchSpan): Tally {
  let total = 0
  let ordinal: number | null = null
  for (const match of matches) {
    total++
    if (total > COUNT_CAP) {
      // The cap is a *stop*, not a clamp: `total` is left one past it so `capped` and the
      // `999+` label are derivable from the number alone, and the ordinal is dropped because
      // this walk may never have reached the selected match.
      return { total, capped: true, ordinal: null }
    }
    if (ordinal === null && match.from === selection.from && match.to === selection.to) {
      ordinal = total
    }
  }
  return { total, capped: false, ordinal }
}

/**
 * The counter's text.
 *
 * Four states, and they are four different sentences rather than four spellings of a number:
 * `999+` means "more than I will count", `3 of 12` means "here, out of this many", `1 match`
 * means the singular (writing `1 matches` is how a user learns to stop reading the label), and
 * `12 matches` is the total with the caret nowhere near one of them — which is the state the bar
 * is in the moment a query is typed and before anything is found.
 */
export function countLabel(counted: Tally): string {
  if (counted.capped) return `${COUNT_CAP}+`
  if (counted.ordinal !== null) return `${counted.ordinal} of ${counted.total}`
  return counted.total === 1 ? '1 match' : `${counted.total} matches`
}
