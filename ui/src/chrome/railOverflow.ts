/**
 * How many activity-rail buttons fit, and therefore which ones are not on screen.
 *
 * > *"left bar icons - i doesn't like to stuck there all extensions (especially language like
 * > YAML, Proto, etc) but also there is a problem - when height of the window is small"*
 *
 * The rail is a flex column with `flex: none` items — a shrunk button would break the
 * measurements `chrome/layoutAudit.ts` asserts, and `ActivityRail.module.css` says so — inside
 * an `overflow: hidden` app shell. So an overfull rail **clips**, exactly as the tab strip
 * clips, and a clipped rail button is not merely ugly: it is unreachable. That is worse here
 * than it is on the strip, because a button is the *only* way into a panel an extension
 * contributes — there is no command for an `ext:` view and no other route in.
 *
 * # This is not an extensions problem
 *
 * It reads like one and it is not, which is worth stating before somebody "fixes" it by
 * capping the extension count. With nothing installed the rail draws ten buttons:
 *
 * ```
 * 10 × 32px  +  9 × 4px gap  +  2 × 8px padding  =  372px
 * ```
 *
 * and the smallest window cide will open is `MIN_HEIGHT` = 430px, of which the header takes 38
 * and the status bar 26, leaving **366**. The rail is already six pixels overfull at the
 * minimum legal window size before anybody installs anything, and `--ui-scale` above 1 makes it
 * worse rather than better: the header and the status bar scale, the 32px button and the 4px
 * gap do not. Each contributed sidebar panel then costs a further 36px.
 *
 * # Why this is arithmetic in its own file
 *
 * The same reason `tabOverflow.ts` is, and that file's header is the long version. Everything
 * that can be wrong here is a number, this repository has no browser and no jsdom, and a rule
 * that lives inside a `useLayoutEffect` is a rule nothing in `ui/scripts/` can execute.
 * `check-rail-overflow.mjs` compiles this file with a bare `tsc` and runs it.
 *
 * **Import nothing — not even a type.** A value import (React, a CSS module, `@/ipc/client`)
 * is how that stops working, and the `@/` alias is not resolvable by a bare compiler at all.
 *
 * # Why counts, and not measured boxes
 *
 * `tabOverflow.clippedTabs` takes the real `offsetLeft` of every tab, because a tab's width
 * depends on its filename, its badge, and whether its buffer is dirty — "about eight fit" is a
 * sentence, not a predicate. A rail button is 32px whatever is in it, so the honest input here
 * is a count and two lengths, and taking measured boxes instead would invite a feedback loop:
 * hiding a button changes the boxes, which changes the answer, which changes what is hidden.
 * Because [`railFit`] is a function of the *container* and of constants only, hiding a button
 * cannot change its own input, and the layout settles in one pass.
 */

/** What the rail has to place, and how much room it has. */
export interface RailFit {
  /**
   * Usable height of the rail's content box — `clientHeight` minus its vertical padding.
   *
   * Zero or negative before the first layout, which [`railFit`] answers by hiding nothing. That
   * is the safe direction and the same one `GitDiffPane`'s `onScreen` starts on: a rail that
   * briefly draws every button costs a frame, and one that briefly hides them all is a rail
   * with no navigation in it.
   */
  readonly available: number
  /** One button's height. `.item` is a fixed 32px square. */
  readonly item: number
  /** The gap between two buttons — `--sp-2`, 4px. */
  readonly gap: number
  /** Buttons that may be collapsed into the menu, in rail order. */
  readonly flexible: number
  /**
   * Buttons that are drawn whatever happens: Settings, and the tool-window toggle below it.
   *
   * They are never collapsed, and the reason is not that they are special — it is that the
   * overflow menu is reached *through* the rail, so a rail whose last controls could be
   * clipped could clip the way out of its own overflow.
   */
  readonly pinned: number
}

/**
 * How many of the flexible buttons are drawn. The rest belong in the overflow menu.
 *
 * Returns `flexible` when everything fits, in which case no overflow control is drawn at all
 * and the rail is bit-identical to what it has always been — which is what keeps
 * `CIDE_AUDIT=1`'s rail measurements (taken at a forced 1440×900) unchanged.
 */
export function railFit({ available, item, gap, flexible, pinned }: RailFit): number {
  // Before the first layout, and against a nonsense measurement: draw everything. See
  // `available` above for why this is the safe direction rather than the lazy one.
  if (!(available > 0) || !(item > 0)) return flexible

  /** Whether `n` buttons stacked with `n - 1` gaps between them fit. */
  const fits = (n: number): boolean => n <= 0 || n * item + (n - 1) * gap <= available

  if (fits(flexible + pinned)) return flexible

  /*
   * Something has to go, so the overflow control appears — and it occupies a slot of its own,
   * which is why it is counted here rather than added afterwards. Adding it afterwards is the
   * off-by-one that puts the last button back under the status bar: the rail would compute a
   * fit for exactly the buttons it has room for, then draw one more thing.
   */
  let shown = flexible
  while (shown > 0 && !fits(shown + 1 + pinned)) shown--
  return shown
}

/**
 * The overflow control's name, which is also its tooltip.
 *
 * A count in the name and never on the glyph — `tabOverflow.overflowHint` makes the same point
 * for the same reason: a label that grew a digit would change the control's size, which changes
 * what fits, which changes the digit.
 */
export function railOverflowHint(hidden: number): string {
  return hidden === 1 ? '1 panel not in view' : `${hidden} panels not in view`
}
