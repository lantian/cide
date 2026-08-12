/**
 * Turning a font size the user chose into the three numbers the code surfaces need.
 *
 * # Why this is not one number
 *
 * `tokens.css` ships `--fs-code: 12.5px`, `--lh-code: 21px` and `--term-line-height: 1.27`,
 * and the comment there explains that the last of those **cannot be `calc()`ed**: xterm.js's
 * `lineHeight` is a multiplier on the *measured glyph box*, not on the font size, and it
 * computes `cell = floor(ceil(fontSize × boxEm × dpr) × lineHeight)` with a `ceil` that
 * happens inside xterm against a measurement no stylesheet can see. So a font size the user
 * picks cannot simply be written into a token and left to cascade: two of the three values
 * have to be re-derived, and one of them needs `devicePixelRatio`.
 *
 * That is the whole reason the two Settings controls did nothing. It was not an unwired
 * `onChange` — the settings were stored correctly and read by nobody, because "read" here
 * means this calculation, and it did not exist.
 *
 * # Why the two sizes are separate but default equal
 *
 * `Settings` offers an editor size and a terminal size, so they are separate — a promise the
 * UI makes and this honours. But `tokens.css` deliberately unified them, because a terminal
 * beside an editor in one tab at a half-pixel difference "reads as two different fonts", which
 * is a bug this project has already shipped once. Both therefore default to the same value, so
 * the out-of-box look is the unified one and divergence is something a user chooses.
 *
 * Pure and import-free, so `check-fonts.mjs` compiles it standalone.
 */

/** The mock's leading over its font size: 21px at 12.5px. Kept as the ratio, not the pair. */
export const LEADING_RATIO = 21 / 12.5

/**
 * JetBrains Mono's glyph box, in ems.
 *
 * hhea ascent 1020 / descent -300 over a 1000 upem, and OS/2 sets `USE_TYPO_METRICS`, so all
 * three metric sets agree at 1.32em. This is a property of the bundled face; a build that
 * changes the mono font has to change this with it, which is why it is named rather than
 * inlined into the arithmetic below.
 */
export const GLYPH_BOX_EM = 1.32

/** What a font size may be set to. Below 6 the UI is unreadable; above 40 one row fills a pane. */
export const MIN_FONT_SIZE = 6
export const MAX_FONT_SIZE = 40

export interface CodeMetrics {
  /** `--fs-code`, in px. */
  fontSize: number
  /** `--lh-code`, in px — what CSS uses. */
  lineHeight: number
  /** `--term-line-height` — xterm's multiplier, which is a different number for the same leading. */
  termLineHeight: number
}

export function clampFontSize(size: number): number {
  if (!Number.isFinite(size)) return 12.5
  return Math.min(MAX_FONT_SIZE, Math.max(MIN_FONT_SIZE, size))
}

/**
 * The three values for one font size.
 *
 * `dpr` is a parameter rather than a read of `window.devicePixelRatio` so this stays pure and
 * so the test can cover the fractional-DPI case, which is the one that goes wrong: at dpr 1.25
 * the `ceil` inside xterm lands on a different integer and a multiplier derived for dpr 1
 * gives a cell one pixel out — visible as the terminal and the editor drifting apart down a
 * long pane, which is exactly the symptom the shared token was introduced to end.
 *
 * The multiplier is *not* clamped to xterm's own minimum of 1: a size where the desired
 * leading is tighter than the glyph box genuinely wants a value below 1, and xterm honours it.
 * What it must not be is zero or negative, which `clampFontSize` already prevents upstream.
 */
export function codeMetrics(size: number, dpr: number): CodeMetrics {
  const fontSize = clampFontSize(size)
  const lineHeight = Math.round(fontSize * LEADING_RATIO)
  // The same arithmetic xterm will do, so the multiplier we hand it produces `lineHeight`.
  const ratio = Number.isFinite(dpr) && dpr > 0 ? dpr : 1
  const box = Math.ceil(fontSize * GLYPH_BOX_EM * ratio)
  return {
    fontSize,
    lineHeight,
    // Guarded against a zero box, which only a degenerate size could produce, because a
    // non-finite multiplier reaches xterm and makes every cell `NaN` wide — a blank pane.
    termLineHeight: box > 0 ? lineHeight / box : 1,
  }
}

/** The custom properties to set, given the two sizes. Returned rather than written, for the test. */
export function fontVariables(
  editorSize: number,
  terminalSize: number,
  dpr: number,
): Record<string, string> {
  const editor = codeMetrics(editorSize, dpr)
  const terminal = codeMetrics(terminalSize, dpr)
  return {
    // `--fs-code` and `--lh-code` are what the editor's stylesheet reads; the terminal reads
    // its own pair because xterm is handed numbers directly rather than cascading.
    '--fs-code': `${editor.fontSize}px`,
    '--lh-code': `${editor.lineHeight}px`,
    '--fs-term': `${terminal.fontSize}px`,
    '--term-line-height': `${terminal.termLineHeight}`,
  }
}
