/**
 * Which tabs the strip is currently hiding, as arithmetic over boxes.
 *
 * > *"When opened files more then current width of window - i doesn't see all other tabs to the
 * > right. Need to create a dropdown icon (like > but down) that will allow to open a list of
 * > opened files that is outside of current view."*
 *
 * The strip clips rather than scrolls or squashes, and it says so in three places:
 * `TabStrip.module.css`'s `.tabs` is `overflow: hidden`, its `.tab` is `flex: none` ("a shrunk
 * tab would eat its own padding and break the widths the audit measures"), and the awaiting
 * marker's paragraph admits the cost — "roughly two fewer tabs fit before the last one is cut
 * off". `useTabDrag.ts` names the same fact from the other side: a clipped tab is neither
 * painted nor reachable. So the tabs past the edge are not merely ugly, they are *gone*, and
 * the answer the user asked for is a list of them.
 *
 * This module is the one *decision* in that feature: given where each tab's box actually is and
 * how much of the box is visible, which ids can the user not see. Everything else — measuring,
 * opening a menu, activating — is DOM and React, and this harness has neither a browser nor a
 * jsdom. So the rule lives here as a pure function over numbers and `ui/scripts/
 * check-tab-overflow.mjs` compiles this file with a bare `tsc` and runs it. The sibling
 * arrangement of `tabDrag.ts`, `menus/model.ts` and `sidebar/treeDrag.ts`, for the reason all
 * three state: a rule that lives inside an event handler is a rule nothing in this repo can
 * execute, and this project has paid for that repeatedly.
 *
 * **Import nothing — not even a type.** `check-tab-overflow.mjs` runs `tsc` over this file
 * alone; a value import (React, a CSS module, `@/ipc/client`) is how that stops working, and the
 * `@/` alias is not resolvable by a bare compiler at all. The structural `TabBox` below is what
 * lets the caller pass real DOM measurements without this file knowing what an `HTMLElement` is.
 *
 * # Why measured geometry rather than a tab count
 *
 * Because no count is right. A tab's width depends on its filename, on whether it is mono
 * (`.shapeFile`) or not (`.shapeClaude`, `.shapeSettings`), on the language badge's two-to-four
 * letters, on the swatch, and on the close button swapping a 12px `×` for a 14px `•` the moment
 * a buffer goes dirty. Every tab also carries the 16px awaiting reserve. "About eight fit"
 * is a sentence, not a predicate, and a chevron that appears when nothing is clipped is exactly
 * as wrong as one that stays hidden when three tabs are.
 */

/**
 * One tab's box, as the DOM reports it.
 *
 * `left`/`width` are `offsetLeft`/`offsetWidth` against the tablist — which is the offset parent
 * because `.tabs` is `position: relative`, the same line `useTabDrag.caretOffset` depends on.
 * Structural rather than a DOM type so this file imports nothing; `TabStrip.tsx` is where the
 * real elements are read.
 */
export interface TabBox {
  readonly id: string
  readonly left: number
  readonly width: number
}

/** The visible window into the strip: how wide it is, and how far it has been scrolled. */
export interface StripGeometry {
  /** `clientWidth` of the tablist. Zero before the first layout — see [`clippedTabs`]. */
  readonly viewport: number
  /**
   * `scrollLeft` of the tablist.
   *
   * A real input, not defensive padding. `overflow: hidden` still makes a scroll container, and
   * the engine scrolls one programmatically when focus moves into it — Tab-focusing a clipped
   * tab's `<button>` does exactly that in the shipped app. When it happens the hidden tabs are
   * the *leading* ones, so a detector that only looked at the right-hand edge would report an
   * empty list on a strip whose first three tabs had gone.
   */
  readonly scroll: number
}

/**
 * How far a tab may hang over an edge and still count as fully visible.
 *
 * One pixel, because the inputs are `offsetLeft`/`offsetWidth`, which WebKit rounds to integers
 * — the same rounded measurement `useTabDrag` works in. A tab overhanging by a rounded pixel is
 * not something a user can see, and reporting it would put a chevron on a strip that looks
 * complete, which is the failure mode that makes a control feel broken: it is *on* and its menu
 * lists a tab the user is looking at.
 *
 * Named rather than written as a literal at the two comparisons below: it is the same fact
 * twice, and two literals is how one of them drifts.
 */
export const EDGE_TOLERANCE_PX = 1

/**
 * The ids of the tabs that are not fully visible, in strip order.
 *
 * "Not *fully* visible" rather than "not visible at all", and that is the whole point: the tab
 * the strip cuts in half is the one the user complained about. Its name is unreadable and its
 * close button is past the edge, so it is as unreachable as the ones beyond it.
 *
 * An unlaid-out strip reports **nothing**. React commits before layout runs, so the first
 * measurement of a fresh window sees `clientWidth === 0`; with no guard, every tab is "clipped"
 * and the chevron flashes on for a frame on every window open — a control that appears and
 * vanishes without the user doing anything is one they stop trusting. Zero-width is not a state
 * with an answer, so the honest return is the empty list.
 *
 * **The pinned console is not special-cased.** It is listed like any other tab when it is
 * clipped. Its pin is about *closing* and *moving* — `menuModel.closable`, `tabDrag.movable`,
 * and `cide_core::workspace` behind both — and neither has anything to do with reaching it. A
 * scrolled strip whose console has gone off the left edge is precisely when a user most needs
 * the list to contain it.
 */
export function clippedTabs(boxes: readonly TabBox[], geo: StripGeometry): string[] {
  if (geo.viewport <= 0) return []

  const from = geo.scroll - EDGE_TOLERANCE_PX
  const to = geo.scroll + geo.viewport + EDGE_TOLERANCE_PX

  const hidden: string[] = []
  for (const box of boxes) {
    const visible = box.left >= from && box.left + box.width <= to
    if (!visible) hidden.push(box.id)
  }
  return hidden
}

/**
 * The sentence on the chevron — its tooltip and its accessible name.
 *
 * A pure function so the singular can be pinned by a check, for the reason `awaitingHint` is one:
 * `1 tabs are not in view` is exactly the kind of thing that ships, because the only way to see
 * it is to have exactly one tab clipped at the moment you happen to look.
 *
 * The count lives *here* and never on the glyph. `▾ 3` would change width at every digit
 * boundary, which changes the tabs box's width, which changes what is clipped — a feedback loop
 * through the reflow-under-the-pointer rule this strip's stylesheet spends a paragraph
 * forbidding.
 */
export function overflowHint(count: number): string {
  return count === 1 ? '1 tab is not in view' : `${count} tabs are not in view`
}
