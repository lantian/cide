/**
 * The list navigation both overlays share.
 *
 * Pulled out because the two lists must feel identical — the same wrap-around, the same
 * Page Up jump, the same Escape — and because "identical" written twice is two things that
 * drift. Nothing here touches the DOM or React, so it is exercised by
 * `ui/scripts/check-picker.mjs` against plain objects.
 *
 * Note what is *not* here: these keys are handled on the input element, not through the key
 * gate. Arrows, Enter and Escape are unbound in the shipped keymap, so the gate passes them
 * through untouched and they arrive at the focused field in the ordinary way. Routing them
 * through the gate instead would mean teaching it which surface has focus, for keys no user
 * will ever rebind.
 */

/**
 * How many rows a Page Up / Page Down moves.
 *
 * A fixed jump, not a measured page: the card's list is `flex: 1` inside a `100vh`-relative
 * max-height, so a real page is whatever the window is tall — about 26 rows at 900px and 44 at
 * 1400px. Measuring it would make the same keystroke move a different distance on two
 * machines, and the virtualizer would have to be consulted from a module that deliberately
 * knows nothing about the DOM. Ten rows is a deliberate under-shoot: it always lands inside
 * the visible list, so the selection never jumps somewhere the user cannot see it arrive.
 */
export const PAGE_ROWS = 10

/** The subset of a keyboard event this module reads. */
export interface ListKey {
  key: string
  shiftKey: boolean
  altKey: boolean
  ctrlKey: boolean
  metaKey: boolean
}

/** What the caller should do. `none` means the key was not ours — let the field have it. */
export type ListAction =
  | { kind: 'none' }
  | { kind: 'select'; index: number }
  | { kind: 'dismiss' }
  /** Enter, with the modifier that was held. The overlay decides what each one means. */
  | { kind: 'accept'; modifier: 'plain' | 'shift' | 'alt' | 'ctrl' }

/**
 * Resolve a keystroke against a list of `count` rows with `selected` currently highlighted.
 *
 * Movement wraps. That is the picker convention — one Up from the top row is the last
 * result, which is how you reach the bottom of a long list without holding a key — and it
 * costs nothing to state here rather than clamping in two components.
 */
export function listAction(ev: ListKey, count: number, selected: number): ListAction {
  const move = (delta: number): ListAction => {
    if (count === 0) return { kind: 'select', index: 0 }
    // `%` on a negative left operand is negative in JS, so the count is added back before
    // the second modulo. Without it, Up from row 0 selects -1 and the list renders empty.
    const next = (((selected + delta) % count) + count) % count
    return { kind: 'select', index: next }
  }

  switch (ev.key) {
    case 'ArrowDown':
      return move(1)
    case 'ArrowUp':
      return move(-1)
    case 'PageDown':
      return move(PAGE_ROWS)
    case 'PageUp':
      return move(-PAGE_ROWS)
    case 'Home':
      return { kind: 'select', index: 0 }
    case 'End':
      return { kind: 'select', index: Math.max(0, count - 1) }
    case 'Escape':
      return { kind: 'dismiss' }
    case 'Enter':
      if (count === 0) return { kind: 'none' }
      if (ev.shiftKey) return { kind: 'accept', modifier: 'shift' }
      if (ev.altKey) return { kind: 'accept', modifier: 'alt' }
      // Meta as well as Ctrl: the mock's footer draws `⌘⏎`, and on Linux that binds to
      // Ctrl (§2 of the plan). Honouring both means the chip is right on either platform.
      if (ev.ctrlKey || ev.metaKey) return { kind: 'accept', modifier: 'ctrl' }
      return { kind: 'accept', modifier: 'plain' }
    default:
      return { kind: 'none' }
  }
}

/** True for the keys `listAction` claims, so the caller knows to call `preventDefault`. */
export function isListKey(action: ListAction): boolean {
  return action.kind !== 'none'
}
