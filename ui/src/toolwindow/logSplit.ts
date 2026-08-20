/**
 * The divider between the Log tab's commit list and its details pane.
 *
 * Import-free for the reason `toolWindowHeight.ts` is: `ui/scripts/check-toolwindow.mjs` compiles
 * it standalone and executes it, and a rule that lives in a React state updater is a rule no
 * check script can run.
 *
 * # Why the split is stored in per mille, not as a fraction
 *
 * `ToolWindowState::log_split` is a `u16` in 0–1000, and this module speaks the same units all
 * the way through so nothing has to remember where the conversion happens. Rust's reasons are
 * that `Workspace` derives `Eq` — which an `f32` poisons down the whole chain — and that a float
 * in a persisted document does not round-trip through `serde_json` exactly. Per mille rather than
 * percent because a 900px panel divided into a hundred steps is a 9px jump, which is visible
 * while dragging.
 *
 * # Why the clamp is done in pixels and handed back in per mille
 *
 * The two minima below are pixel facts about what fits — an oid, a subject, an author and a date
 * on one row; a commit message and a file list beside it. A fraction means one thing at 1800px
 * and something else at 700px, so clamping the fraction directly would let the details pane
 * vanish on a narrow window while the stored number still looked reasonable. [`clampLogSplit`]
 * therefore converts to pixels, applies both floors there, and converts back.
 */

/** The commit list's default share, in per mille. Mirrors `ToolWindowState::default()`. */
export const LOG_SPLIT_DEFAULT = 450

/**
 * A History tab's default share: **half**.
 *
 * > *"such git log panel (when opening from 'Show history for this file') should be devided by
 * > 50%/50% … coz we currently checking diff of one file"*
 *
 * The reason is the one in that sentence and it is about the *question*, not about taste. The Log
 * tab's right half is a commit's message and its file list — a narrow column beside a wide list,
 * which is what 450 is for. A History tab's right half is one file's diff, which is the thing the
 * tab was opened to read, and a diff in a 420px column wraps every line.
 */
export const LOG_SPLIT_HISTORY = 500

/**
 * The narrowest useful commit list, in CSS pixels.
 *
 * 260 is a short oid (8ch), a subject, an author and a relative date on one 22px row with the
 * subject still readable. Below it the subject — the only column anyone scans — is the one that
 * truncates, because it is the flexible one.
 */
export const LOG_LIST_MIN_PX = 260

/**
 * The narrowest useful details pane, in CSS pixels.
 *
 * 320 because it holds a wrapped commit message and a file list whose rows are a status letter, a
 * path and a `+12 −3` figure. `--w-log-details` (420px) is what it *starts* at; this is where it
 * stops shrinking.
 */
export const LOG_DETAILS_MIN_PX = 320

/**
 * Below this total width the pane stacks instead of splitting, and **the divider is not drawn at
 * all**.
 *
 * `LOG_LIST_MIN_PX + LOG_DETAILS_MIN_PX` is 580, so between 580 and this there is a legal split
 * — but a 260px list beside a 320px details pane is two columns that are both too tight to read,
 * which is worse than one of each stacked. The same shape as `DIFF_SPLIT_MIN_PX` in
 * `panes/GitDiffPane.tsx`, which falls back to unified in a narrow pane for the same reason: a
 * layout that technically fits is not a layout that works.
 *
 * A divider that cannot usefully be dragged is not rendered, rather than rendered inert. An inert
 * `role="separator"` is a keyboard stop that does nothing.
 */
export const LOG_STACK_BELOW_PX = 640

/** How far an arrow key moves the divider, in per mille. Matches the splitter's 16px feel. */
export const LOG_SPLIT_KEY_STEP = 20

/** How the Log tab lays itself out at this width. */
export interface LogLayout {
  readonly mode: 'split' | 'stacked'
  /** The commit list's size: a width when split, a height when stacked. `0` means "let it flow". */
  readonly list: number
}

/**
 * A split brought inside both minima, in per mille.
 *
 * A non-finite input returns the default rather than propagating `NaN` into a `grid-template`,
 * where the declaration is simply invalid and nothing says so — the same failure
 * `clampToolWindowHeight` guards against.
 *
 * When the width is too narrow to honour both floors the answer is the default: the caller is
 * about to stack anyway ([`logLayout`]), and returning some pinned extreme would mean the stored
 * value silently changed because the user resized their window.
 */
export function clampLogSplit(perMille: number, width: number): number {
  if (!Number.isFinite(perMille)) return LOG_SPLIT_DEFAULT
  if (!Number.isFinite(width) || width < LOG_LIST_MIN_PX + LOG_DETAILS_MIN_PX) {
    return Math.round(Math.max(0, Math.min(1000, perMille))) || LOG_SPLIT_DEFAULT
  }
  const px = (perMille / 1000) * width
  const bounded = Math.max(LOG_LIST_MIN_PX, Math.min(px, width - LOG_DETAILS_MIN_PX))
  return Math.round((bounded / width) * 1000)
}

/** What a drag has made the split, given where it started and how far the pointer moved. */
export function splitFromDrag(originPerMille: number, dx: number, width: number): number {
  if (!Number.isFinite(width) || width <= 0) return clampLogSplit(originPerMille, width)
  return clampLogSplit(originPerMille + (dx / width) * 1000, width)
}

/** The value an arrow key produces. Positive `steps` grows the list. */
export function splitFromKey(perMille: number, steps: number, width: number): number {
  return clampLogSplit(perMille + steps * LOG_SPLIT_KEY_STEP, width)
}

/** How to lay the Log tab out, and how big to make the list. */
export function logLayout(width: number, perMille: number): LogLayout {
  if (!Number.isFinite(width) || width < LOG_STACK_BELOW_PX) return { mode: 'stacked', list: 0 }
  return { mode: 'split', list: Math.round((clampLogSplit(perMille, width) / 1000) * width) }
}
