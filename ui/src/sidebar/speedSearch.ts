/**
 * Speed search: which keystroke does what, while the user is typing into a sidebar tree.
 *
 * Type into the explorer or the changes tree and it starts filtering to rows whose *name*
 * contains what you typed; Down and Up walk the matches, Escape leaves. IDEA's gesture, and
 * the request named IDEA.
 *
 * # What is here and what is deliberately not
 *
 * Here: the **key rule** — which keystroke extends the query, which navigates, which exits,
 * which is handed straight back — plus wrap-around and expiry. Not here: the **matching rule**,
 * which lives in `crates/cide-fs/src/speed.rs` and reaches both trees through two commands
 * (`fs_tree_match` and `tree_match_labels`) in the shape `cmd::picker::picker_rank` already
 * established. That module's header argues the split at length; the short version is that the
 * explorer's rows are not in the webview — a match may be at row 40,000 of a flattening Rust
 * owns with rows 300–500 resident — so something in Rust has to walk the tree, and once that
 * exists a TypeScript copy for the other tree is a second answer to a settled question.
 *
 * Pure and **import-free**, the same arrangement as `clickSemantics.ts`, `treeStatus.ts` and
 * `rowWindow.ts`: there is no JS test runner here, so `check-speed-search.mjs` compiles this
 * one module with a bare `tsc` and imports the output under node. Do not add an import.
 *
 * # Why this is not a command in the registry
 *
 * The same argument `treeKeyAction` makes in full one file over, and this is the sharper case.
 * A binding on a bare letter is **focus-scoped**, not context-scoped: `sidebarFiles` means the
 * panel is *visible*, which it is while the user types in a terminal beside it, and the key
 * gate's window-capture listener resolves a global binding before the event reaches its target
 * — so a registry binding on `t` would eat that letter in every pane in the window. A handler
 * on the focused element is asked exactly when the question is "does this tree have the
 * caret". `cide_core::keymap`'s `no_default_binds_an_unmodified_printable_key` is the other
 * half of the same rule, holding the default layer to it from the Rust side.
 */

/** How long an idle query stays armed. See [`expired`]. */
export const SPEED_TIMEOUT_MS = 2000

/** The modifiers, in the shape both trees already build for `keySelect`. */
export interface SpeedMods {
  readonly ctrl: boolean
  readonly meta: boolean
  readonly alt: boolean
  readonly shift: boolean
}

/**
 * What a keystroke does to a speed search.
 *
 * `pass` and `exitThenPass` are two different things and the difference is the whole safety
 * argument: `pass` means the tree never knew a search was happening, `exitThenPass` means the
 * search ends *and* the tree still performs the key's ordinary job. Collapsing them into one
 * "not handled" would leave the query armed under a fold that renumbered every match.
 */
export type SpeedAction =
  /** Not ours. The tree handles it exactly as it did before this feature existed. */
  | { readonly kind: 'pass' }
  /** Append a character to the query. Starts a search when there was none. */
  | { readonly kind: 'extend'; readonly char: string }
  /** Remove the last character. Removing the last one at all ends the search. */
  | { readonly kind: 'erase' }
  /** Walk the match list. */
  | { readonly kind: 'move'; readonly delta: 1 | -1 }
  /** Open the row the cursor is on, then leave. */
  | { readonly kind: 'accept' }
  /** Leave, and swallow the key. */
  | { readonly kind: 'exit' }
  /** Leave, then let the tree do what it always does with this key. */
  | { readonly kind: 'exitThenPass' }
  /** Swallow it and do nothing at all. Reserved for keys that would destroy something. */
  | { readonly kind: 'swallow' }

const PASS: SpeedAction = { kind: 'pass' }
const ERASE: SpeedAction = { kind: 'erase' }
const EXIT: SpeedAction = { kind: 'exit' }
const EXIT_THEN_PASS: SpeedAction = { kind: 'exitThenPass' }
const SWALLOW: SpeedAction = { kind: 'swallow' }
const ACCEPT: SpeedAction = { kind: 'accept' }
const NEXT: SpeedAction = { kind: 'move', delta: 1 }
const PREV: SpeedAction = { kind: 'move', delta: -1 }

/**
 * Whether `key` is one printable character.
 *
 * `KeyboardEvent.key` is the *produced* character for a printable key and a name — `ArrowDown`,
 * `F2`, `Escape` — for everything else, so "length is one" is the whole test and it is the one
 * every type-ahead implementation uses. `Tab` and `Enter` are names, so they are excluded for
 * free rather than by a list somebody has to maintain.
 *
 * A space is printable and is handled separately by the caller; see [`speedKey`].
 */
export function isTypedCharacter(key: string): boolean {
  // `[...key].length` rather than `key.length`, so an astral character — a keyboard layout
  // producing an emoji, or a CJK IME committing one — is one character rather than two UTF-16
  // units that fail the test and fall through to the tree's navigation.
  return [...key].length === 1
}

/**
 * What this keystroke means, given whether a search is running and what has been typed.
 *
 * `query` rather than the whole state, because that is all the decision needs and a smaller
 * argument is a smaller thing for the two call sites to get wrong. An **empty** query means no
 * search is running: it is the state before the first character and the state after the last
 * Backspace, and there is no third.
 *
 * # The table, and what each row is protecting
 *
 * | key | inactive | active | why |
 * | --- | --- | --- | --- |
 * | printable | starts a search | extends it | the gesture |
 * | Space | `pass` | extends **only** with a query already typed | Space is the tick in the changes tree, and a bare Space must stay the tick |
 * | ↓ ↑ | `pass` | next / previous match | |
 * | Escape | `pass` | `exit` | must run *before* the file tree's clipboard branch, or Escape throws away a pending cut instead of ending a search |
 * | Backspace | `pass` | `erase` | |
 * | Delete | `pass` | **`swallow`** | `treeKeyAction` turns a bare Delete into *Move to Trash*. A user mid-word must never trash a file |
 * | Enter | `pass` | `accept` | |
 * | ← → Home End | `pass` | `exitThenPass` | a fold renumbers every match index, so the search has to be gone before the fold happens |
 * | any modified chord | `pass` | `exitThenPass` | Ctrl+C, Ctrl+X, Ctrl+V, Ctrl+A and Ctrl+R all still do exactly what they did |
 * | anything else (F2, Tab, …) | `pass` | `exitThenPass` | |
 *
 * The two rows in bold type are the ones this function exists for. Everything else could have
 * been written inline in a handler; *Delete must not delete* and *Escape must not reach the
 * clipboard* are decisions, and a decision inside an event handler is a decision no check
 * script can compile.
 */
export function speedKey(query: string, key: string, mods: SpeedMods): SpeedAction {
  const active = query.length > 0

  /*
   * A modified chord is never speed search's, in either state.
   *
   * Not even Ctrl+Backspace — "erase a word" is a text-field gesture and this is a filter over
   * a tree, so taking it would be inventing a behaviour rather than honouring one. And this
   * clause is what keeps Ctrl+C/X/V, Ctrl+A and Ctrl+R doing exactly what they do today: the
   * search ends, and the tree's own handler runs untouched.
   *
   * Shift is deliberately **not** in the list. Shift+A is how a capital `A` is typed, and a
   * speed search that could not spell `App.tsx` would be a curiosity rather than a feature.
   */
  if (mods.ctrl || mods.meta || mods.alt) return active ? EXIT_THEN_PASS : PASS

  switch (key) {
    case 'Escape':
      return active ? EXIT : PASS
    case 'Backspace':
      return active ? ERASE : PASS
    case 'Delete':
      // Swallowed, not passed. `treeKeyAction` answers a bare Delete with `'delete'`, which
      // opens the trash confirmation over whatever is selected — and the selection is wherever
      // the search just moved it. Typing `notes` and reaching for Backspace with the wrong
      // finger must not put a delete dialog on screen.
      return active ? SWALLOW : PASS
    case 'ArrowDown':
      return active ? NEXT : PASS
    case 'ArrowUp':
      return active ? PREV : PASS
    case 'Enter':
      return active ? ACCEPT : PASS
    case 'ArrowLeft':
    case 'ArrowRight':
    case 'Home':
    case 'End':
    case 'PageUp':
    case 'PageDown':
      // Leave first. Left and Right fold, Home and End jump — and a fold re-flattens the tree,
      // which renumbers every index in the match list. Exiting first means the list is gone
      // before it can be wrong.
      return active ? EXIT_THEN_PASS : PASS
    case ' ':
      /*
       * A space continues a filename and is also the changes tree's tick.
       *
       * With something typed it is part of the query — `check tree` has one — and with nothing
       * typed it is the tick, unchanged. This is the one key whose meaning depends on the
       * query rather than on the search merely being active, which is why `speedKey` takes the
       * query rather than a boolean.
       */
      return active ? { kind: 'extend', char: ' ' } : PASS
    default:
      break
  }

  if (isTypedCharacter(key)) return { kind: 'extend', char: key }
  // A named key nobody above claimed: F2, Tab, Insert, a dead key. The search ends and the
  // tree does whatever it does — F2 is deliberately unclaimed there, Tab moves focus out.
  return active ? EXIT_THEN_PASS : PASS
}

/**
 * The next match index, wrapping.
 *
 * Wrapping rather than clamping, because the list is short and cyclic is what every
 * find-next in this application already does (`listKeys.ts`, the find bar's ordinal). A clamp
 * would make the last Down silently do nothing, which in a feature whose entire feedback is a
 * counter reads as the key having stopped working.
 *
 * `-1` for an empty list: there is no match to be on, and returning `0` would let a caller
 * index into nothing.
 */
export function nextMatch(count: number, at: number, delta: number): number {
  if (count <= 0) return -1
  const from = at < 0 ? (delta > 0 ? -1 : 0) : at
  return (((from + delta) % count) + count) % count
}

/**
 * Whether an armed query is stale.
 *
 * Decided by **comparing timestamps on the next keystroke**, not by a timer firing — the same
 * arrangement, and the same reason, as `keys/gate.ts`'s prefix machine: a background webview
 * has its timers throttled, so a timer that fires late would leave a stale query armed and the
 * next letter would land in the middle of a word the user typed a minute ago. A timer is still
 * *set*, but only to clear the readout; expiry itself is this function.
 *
 * 2000 ms rather than the gate's 1000, because a filename is longer than a chord and the pause
 * between `Fil` and `eTree` is a pause to look at the list.
 */
export function expired(typedAt: number, now: number): boolean {
  return now - typedAt >= SPEED_TIMEOUT_MS
}

/**
 * The query after this action, or `null` when the search is over.
 *
 * Small enough to inline at both call sites and therefore exactly the kind of thing that ends
 * up spelled two ways: the explorer erasing the last character and *keeping* an empty search
 * armed while the changes tree ends it would be two features wearing one name.
 */
export function applyKey(query: string, action: SpeedAction): string | null {
  switch (action.kind) {
    case 'extend':
      return query + action.char
    case 'erase': {
      const next = query.slice(0, -1)
      // The last Backspace ends the search rather than leaving an empty one armed. An empty
      // query matches nothing (`Needle::new` refuses it), so an armed empty search is a state
      // in which every key is swallowed and no row is highlighted — indistinguishable, from
      // the user's side, from the tree having frozen.
      return next.length === 0 ? null : next
    }
    case 'exit':
    case 'exitThenPass':
    case 'accept':
      return null
    default:
      return query.length === 0 ? null : query
  }
}

/**
 * What the overlay says, under the query.
 *
 * Three states and all three are said out loud, because the two dead ends are the ones a user
 * reads as breakage:
 *
 *   * **no match in the expanded tree** — not `no match`. Speed search only sees what is
 *     expanded (see `cide_fs::speed`), so a user who knows the file is in the project would
 *     read a bare `no match` as a lie. The sentence names the reason and points at the fix.
 *   * **first 1000 matches** — the Rust cap cut the list. A silently short answer is
 *     indistinguishable from a tree that does not contain the rest.
 *   * `3 of 17` otherwise, which is the whole feedback for the matches that are not on screen —
 *     and in a windowed tree, most of them are not.
 */
export function speedSummary(count: number, at: number, truncated: boolean): string {
  if (count === 0) return 'no match in the expanded tree'
  const position = `${at < 0 ? 1 : at + 1} of ${count}`
  return truncated ? `${position} — first ${count} matches` : position
}

/**
 * Whether a blur really means the tree lost focus, or only that focus moved inside it.
 *
 * React's `onBlur` is the **bubbling** `focusout`, so it fires on the container whenever focus
 * leaves any descendant — including when it moves from one row to the next. Wired directly to
 * `exit`, that made speed search in the git changes tree cancel itself the instant it succeeded:
 * landing on a match calls `.focus()` on the matched row, focus leaves the previously focused
 * row, `focusout` bubbles, and the search that had just found something cleared itself.
 *
 * The explorer did not show it, and that is why a source-grep gate could not: it keeps a single
 * tab stop, so landing on a match moves no DOM focus and no `focusout` is generated. Two trees,
 * one line of markup, opposite behaviour — the difference is a roving `tabindex`, which is
 * invisible to a regex.
 *
 * `related === null` counts as leaving: focus going to the document body, or nowhere at all, is
 * not focus staying in the tree.
 *
 * Takes a structural `contains` rather than an `HTMLElement` so it stays DOM-free and
 * `check-speed-search.mjs` can drive it with a stub — this is the rule, and the rule has to be
 * somewhere a check can run it rather than somewhere a check can only grep for it.
 */
export function blurLeftTheTree(
  container: { contains(node: unknown): boolean } | null,
  related: unknown,
): boolean {
  if (container === null) return true
  if (related === null || related === undefined) return true
  return !container.contains(related)
}
