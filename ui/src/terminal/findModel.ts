/**
 * The terminal find bar's arithmetic and its keyboard: the half that can be got wrong quietly.
 *
 * `panes/TerminalFindBar.tsx` is the DOM and `@xterm/addon-search` is the search; neither can be
 * compiled by `ui/scripts/` — one needs React and a mounted pane, the other a live `Terminal`
 * with a rendered element. What is left over is a counter, a scope note and a three-way verdict
 * on a keystroke, and every one of those is a sentence a user reads. So they live here, this
 * module **imports nothing**, and `ui/scripts/check-terminal-find.mjs` compiles it standalone
 * with a bare `tsc` and runs it under node — the rule `editor/findMatches.ts`, `panes/awaiting.ts`
 * and `terminal/keys.ts` already follow, and for the same reason.
 *
 * # Why the labels are the editor's labels, spelled out again rather than imported
 *
 * `editor/findMatches.ts::countLabel` already decided what a find bar's counter says, and it
 * decided it against a bug worth not re-introducing: a bare total (`12 matches`) makes wrapping
 * past the last match indistinguishable from the key having done nothing, because the number
 * does not move. `3 of 12` is unmistakable. Two find bars in one app that word the same fact
 * differently is a worse outcome than a little duplication, so the four sentences here are
 * *exactly* that function's four sentences.
 *
 * They are not that function, because the two sit on different data and an import would cost
 * more than it saves:
 *
 *  * `countLabel` takes a `Tally` this module can only fabricate — it is the product of a walk
 *    over a CodeMirror document, and xterm hands us a finished `{resultIndex, resultCount}`
 *    instead.
 *  * The caps are different mechanisms. `COUNT_CAP` is where `tally` *stops walking*, so it
 *    leaves `total` one past the cap and derives `capped` from that. `@xterm/addon-search` slices
 *    its result list to `highlightLimit` (`ISearchAddonOptions`), so the count saturates *at*
 *    the limit and there is no "one past" to read.
 *  * An import would end this module's standalone compile, and with it the only gate that runs
 *    this arithmetic at all.
 *
 * The duplication is held together by the check rather than by hope: `check-terminal-find.mjs`
 * compiles `editor/findMatches.ts` alongside this module and asserts the two produce identical
 * strings across the whole cross-product of counts and ordinals. [`FIND_HIGHLIGHT_LIMIT`] is
 * set to `COUNT_CAP` for the same reason — so `999+` means the same number in both bars.
 */

/**
 * How many matches the terminal search highlights and counts before it saturates.
 *
 * Handed to `new SearchAddon({ highlightLimit })`, which is the *only* knob: the addon slices
 * its own result list to this number (`ResultTracker.updateResults`), so it is simultaneously
 * the highlight budget and the counting budget. There is no way to count past it without
 * highlighting past it, and highlighting is a decoration per match on a live terminal.
 *
 * 999, matching `editor/findMatches.ts::COUNT_CAP`, so `999+` means the same thing in the
 * terminal's bar as in the editor's. It is also comfortably above any count a person reads: past
 * a thousand hits the honest answer is "narrow the query", which is what the `+` says.
 */
export const FIND_HIGHLIGHT_LIMIT = 999

/**
 * What `@xterm/addon-search` last reported, as its `onDidChangeResults` delivers it.
 *
 * **`index` is 0-based and is `-1` for "not on a match"** — the addon's own spelling, kept
 * verbatim rather than normalised on the way in, so that a reader comparing this against
 * `ISearchResultChangeEvent` sees the same two numbers. Turning it into an ordinal is
 * [`findCountLabel`]'s job and is done in one place.
 *
 * The event fires **only when decorations are enabled** (`ResultTracker.fireResultsChanged`
 * returns immediately otherwise), which is why the bar always passes a `decorations` block. A
 * count with the highlighting switched off is not a cheaper option; it is no count at all.
 */
export interface FindResults {
  /** Matches found, saturating at [`FIND_HIGHLIGHT_LIMIT`]. */
  readonly count: number
  /** 0-based index of the active match, or `-1` when the selection is not on one. */
  readonly index: number
}

/** Nothing searched yet. The state the bar opens in, and the state a cleared query returns to. */
export const NO_RESULTS: FindResults = { count: 0, index: -1 }

/**
 * The counter's text, or `''` when there is nothing to count yet.
 *
 * The empty string for an empty query is not the same as `0 matches`: the bar opens with an
 * empty field, and a bar that says "0 matches" before you have typed anything is reporting a
 * failure that has not happened. `countLabel` has no such state because CodeMirror's panel is
 * only ever asked about a query that exists.
 */
export function findCountLabel(query: string, results: FindResults): string {
  if (query === '') return ''
  if (results.count >= FIND_HIGHLIGHT_LIMIT) return `${FIND_HIGHLIGHT_LIMIT}+`
  if (results.index >= 0) return `${results.index + 1} of ${results.count}`
  return results.count === 1 ? '1 match' : `${results.count} matches`
}

/**
 * What the bar says about *where* it is searching, or `null` when there is nothing to explain.
 *
 * # The state this exists for
 *
 * A terminal has two buffers. On the normal one, `buffer.active` is the scrollback plus the
 * screen, which is what "search this terminal" obviously means. On the **alternate** one — which
 * is what `\x1b[?1049h` switches a full-screen TUI into, and which the Claude Code console
 * spends most of its life in — there is no scrollback at all: the alternate buffer is exactly
 * the visible screen, by definition, and the transcript above it is not reachable from it.
 *
 * `@xterm/addon-search` searches `buffer.active` and nothing else (checked in the shipped
 * bundle, not assumed). So on the alternate buffer the search still works and still finds
 * things — it just cannot see one line further than the user can. Saying so is the whole point
 * of this function. The alternative was to find nothing quietly, or to say "no results" for a
 * word that is plainly ten lines up in the transcript the user was reading a minute ago, and
 * both of those describe a broken feature rather than a scoped one.
 *
 * # Why not switch the buffer, or search the Rust mirror instead
 *
 * Writing `\x1b[?1049l` to get at the scrollback would take the child's screen away from it
 * mid-turn, which is a change to what the program is drawing in order to look at it — the
 * `layout/paneHosts.ts` class of bug, one layer down. And `cide_pty`'s vt100 mirror holds the
 * same alternate screen for the same reason, so it has no more history to offer; the scrollback
 * a user wants is genuinely not being kept by anybody while a TUI owns the screen. A future
 * feature that searches the *session transcript on disk* is a different feature with a different
 * name, and this note is what stops it being confused with this one.
 */
export function findScopeNote(alternate: boolean): string | null {
  return alternate ? 'visible screen only — this program is drawing a full-screen view' : null
}

/** What a keystroke inside the find field means. `null` is "an ordinary character, let it type". */
export type FindKey = 'next' | 'previous' | 'close' | 'refocus'

/**
 * The parts of a `KeyboardEvent` the find field's rule depends on.
 *
 * Structural, matching `TerminalKeyEvent` next door, so this module needs no DOM lib types and
 * the check can drive it with plain objects.
 */
export interface FindKeyEvent {
  readonly key: string
  readonly shiftKey: boolean
  readonly ctrlKey: boolean
  readonly altKey: boolean
  readonly metaKey: boolean
}

/**
 * Resolve a keystroke typed *into the find field*.
 *
 * Deliberately a small set, and deliberately the editor find bar's set, so that the two bars
 * answer the same fingers: **Enter** is next, **Shift+Enter** is previous, **Escape** closes and
 * gives the terminal back the keyboard, and **Ctrl+F** re-selects the field so a second press
 * of the chord that opened the bar retypes over the old query rather than doing nothing.
 *
 * `F3`/`Shift+F3` are **not** here even though they are find-next/find-previous in the editor.
 * They are `@codemirror/search`'s, scoped to `editor search-panel`, and
 * `cide_core::keymap`'s `nothing_binds_the_find_bars_f_keys` keeps them out of the global keymap
 * precisely so that bar keeps them. Claiming them in *this* field would not collide — a keystroke
 * in a terminal pane never reaches CodeMirror — but it would make F3 mean find-next in two
 * unrelated places and unbindable in both, and nobody asked for it.
 *
 * Ctrl+F is matched here rather than being left to `terminalOpensFind` because that function
 * only ever sees keystrokes delivered to *xterm's own textarea*, and while this field has focus
 * the terminal has none. The two are the same chord answered by whichever surface has the caret,
 * which is what makes the second press feel like one gesture.
 */
export function findFieldKey(ev: FindKeyEvent): FindKey | null {
  if (ev.altKey || ev.metaKey) return null

  if (ev.key === 'Escape' && !ev.ctrlKey && !ev.shiftKey) return 'close'
  if (ev.key === 'Enter' && !ev.ctrlKey) return ev.shiftKey ? 'previous' : 'next'
  if (ev.ctrlKey && !ev.shiftKey && ev.key.length === 1 && ev.key.toLowerCase() === 'f') {
    return 'refocus'
  }
  return null
}
