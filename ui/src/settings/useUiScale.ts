/**
 * The chrome scale, for the surfaces that measure themselves in JavaScript.
 *
 * # Why this exists at all
 *
 * Almost every chrome size follows `--ui-scale` through the cascade, which is the whole point
 * of the token ladder in `tokens.css`: write the property once on `<html>` and 405 rules move.
 * Seven surfaces cannot, because their row height is not a CSS declaration — it is a number
 * handed to `@tanstack/react-virtual`'s `estimateSize`, which uses it to place every row with
 * an absolute `transform` and to size the scroll spacer. CSS never sees it.
 *
 * That is the file tree, the search panel, and the five pickers. Left alone they would keep
 * 24px and 26px rows while the text inside them grew — the text clipping first, and the
 * scrollbar lying about the list's length for as long as it lasted. The file tree is the
 * surface this setting was asked for first, so "the cascade covers it" was not good enough.
 *
 * # Why the store and not `getComputedStyle`
 *
 * Reading the property back off `<html>` would be the same number by a longer route, and a
 * layout read on every render. More importantly it would not *re-render*: a virtualizer keeps
 * its measurements until something tells React the estimate moved, and a DOM property is not
 * something React watches. Selecting the stored size does both jobs — the value is exact
 * because `paintUiScale` writes the property from this very field, and the subscription is
 * what makes a settings change reach a mounted list.
 *
 * A scalar selector, deliberately. `check:selectors` exists because a selector returning a
 * fresh array or object re-renders for ever and ends at *Maximum update depth exceeded*; a
 * number compares by value and cannot.
 */
import { useWorkspace } from '@/store/workspace'

import { UI_BASE_FONT_SIZE, uiScale } from './fontScale'

/** The live multiplier — 1 at the default size, and 1 again before the bootstrap lands. */
export function useUiScale(): number {
  return useWorkspace((s) => uiScale(s.boot?.workspace.settings.uiFontSize ?? UI_BASE_FONT_SIZE))
}

/**
 * A design row height in the scale it should currently be drawn at.
 *
 * Rounded to a whole pixel, and that is not tidiness. A virtualizer multiplies this by the
 * row index to place the row, so a fractional height accumulates: at 24.35px the thousandth
 * row is 350px away from where a `transform` on a subpixel grid actually paints it, and the
 * list drifts out from under the scrollbar. Rounding once here keeps every row on the same
 * integer grid the unscaled code already assumed.
 *
 * Floored at 1 so a degenerate scale cannot produce a zero-height row, which is an infinite
 * list as far as the virtualizer's arithmetic is concerned.
 */
export function scaledRow(designPx: number, scale: number): number {
  return Math.max(1, Math.round(designPx * scale))
}

/**
 * The same multiplier, read off `<html>`, for code that cannot hold a hook.
 *
 * A few surfaces measure themselves from module scope rather than from a component —
 * `toolwindow/ToolWindowSplitter.tsx` paints the panel's height from a plain function so the
 * value is on the document before React's first commit. Those cannot call [`useUiScale`], and
 * giving them a module-level mirror of the setting would be a second copy that goes stale.
 *
 * The property is authoritative, not a cache: `paintUiScale` writes it from the stored size,
 * and `public/theme-boot.js` has already written it in `<head>` from `?ui=`, so it is correct
 * before any module runs. Falls back to 1 rather than to a stored default, because the only way
 * to reach the fallback is a document with no stylesheet — where 1 is what `tokens.css` would
 * have said anyway.
 */
export function currentUiScale(): number {
  const raw = getComputedStyle(document.documentElement).getPropertyValue('--ui-scale')
  const value = Number.parseFloat(raw)
  return Number.isFinite(value) && value > 0 ? value : 1
}
