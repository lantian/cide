/**
 * What the three markdown layouts are, and the numbers around them. (M20)
 *
 * Everything the markdown pane *decides* rather than draws: which layouts exist and in what
 * order, what a narrow pane does to `split`, the size past which a document is not previewed at
 * all, and the divider position.
 *
 * # Why this module imports nothing
 *
 * `ui/scripts/check-markdown.mjs` compiles it standalone — the same reason `highlightLevel.ts`,
 * `autosave.ts` and `codeIntelGate.ts` give. The rules here are the ones a user notices being
 * wrong (a split that will not open, a preview that silently does not appear), and a rule
 * written inside a component is a rule nothing in this repository can run.
 */
import type { MdView } from './types'

/** The layouts, in the order the segmented control draws them. */
export const MD_VIEWS: readonly MdView[] = ['text', 'split', 'preview']

/** The control's tooltips. Short, because they sit on 22px squares. */
export const MD_VIEW_LABELS: Readonly<Record<MdView, string>> = {
  text: 'Editor only',
  split: 'Editor and preview',
  preview: 'Preview only',
}

/**
 * Below this, `split` is overruled to `preview`.
 *
 * `panes/GitDiffPane.tsx` solved this once for side-by-side diffs and the argument is the same:
 * two columns in a 300px pane are two columns nobody can read, and silently giving one is worse
 * than saying so. What differs is the fallback. A diff falls back to `unified` because that is
 * the shape its per-line staging is expressed in; a markdown pane falls back to **preview**,
 * because a user who asked for split asked to see the rendering and the buffer is one click
 * away in the same control.
 */
export const MD_SPLIT_MIN_PX = 520

/**
 * The pane width below which the switch moves out of the top-right corner.
 *
 * The corner is not free. `layout/PaneTitleBar.module.css` floats ⊞ ⛶ ⧉ × there, and on an editor
 * pane it publishes `--pane-corner-clear` — `--w-minimap` (96px) plus `--pane-corner` (125px), so
 * **221px** — as the band a pane's own content must keep clear. `panes/EditorPane.module.css`
 * carries the write-up of the once a strip reserved 125 instead: both of its buttons were drawn
 * inside the cluster's live hit box, hovered, and answered the ×'s click.
 *
 * So the arithmetic is 221 for the band, 72 for three 22px squares and their padding, and about
 * 27 of margin before the switch is touching the left edge of a pane. Below that it goes to the
 * bottom-right, where nothing floats — which is where `panes/ImagePane.tsx` puts its one toggle,
 * for the same reason.
 *
 * `ui/scripts/check-markdown.mjs` re-derives the 221 from the two tokens and fails if this number
 * stops covering it, because the failure mode is not a layout glitch: it is three buttons that
 * are drawn and take a click that belongs to the close button.
 */
export const MD_CONTROL_MIN_PX = 320

/**
 * The size past which a document is shown as text and not previewed.
 *
 * **The editor's own cap, deliberately the same number.** `EditorSurface.tsx`'s
 * `HIGHLIGHT_LIMIT_BYTES` is where a buffer stops being highlighted, and
 * `ui/scripts/check-markdown.mjs` reads that literal out of the source and fails if the two
 * disagree. The reasoning is the one README gives for `MAX_IMAGE_BYTES`: two caps that can drift
 * produce a state where a file is too big for one half of a feature and small enough for the
 * other, and here that state is a split pane with a blank right-hand side and nothing on screen
 * saying why.
 *
 * Bytes, not `String.length` — see `editor/byteSize.ts`. A megabyte of CJK prose is a third of a
 * megabyte of UTF-16 code units, and measuring it in those would preview a file three times
 * larger than this says.
 */
export const PREVIEW_LIMIT_BYTES = 1024 * 1024

/**
 * The size past which one fenced code block is shown uncoloured.
 *
 * A fence is tokenized by driving the language's real `StreamParser` over it (`fenceTokens.ts`),
 * which is the same work the buffer does — but the buffer does it for the *visible* lines only,
 * through CodeMirror's viewport, and the preview has no viewport. A single 200 KB fence would
 * therefore cost more than opening the file did.
 */
export const FENCE_HIGHLIGHT_LIMIT_BYTES = 64 * 1024

/**
 * How many images one document may resolve.
 *
 * Each one is an `image.read`, and each `image.read` grants this webview
 * `asset_protocol_scope().allow_file()` for that path — see `crates/cide-app/src/cmd/file.rs`.
 * The static scope is empty precisely so that permission is granted a file at a time, and a
 * document is a thing an agent can write: a generated `.md` with ten thousand image references
 * would otherwise hand the webview ten thousand grants for having been opened once.
 */
export const MAX_PREVIEW_IMAGES = 64

/**
 * The layout actually used, and whether the pane's width overruled the choice.
 *
 * Both facts travel together for the reason `editor/diffViewMode.ts`'s `DiffViewState` gives
 * about `writable`: the control has to draw the *chosen* mode as pressed and the *overruled* one
 * as explained, and a component that recomputed the second from the first would be a second copy
 * of this rule.
 */
export interface EffectiveView {
  readonly chosen: MdView
  readonly layout: MdView
  readonly overruled: boolean
}

export function effectiveView(chosen: MdView, widthPx: number): EffectiveView {
  const overruled = chosen === 'split' && widthPx > 0 && widthPx < MD_SPLIT_MIN_PX
  return { chosen, layout: overruled ? 'preview' : chosen, overruled }
}

/** The next layout in the cycle. What a single-button fallback control advances through. */
export function nextView(current: MdView): MdView {
  const at = MD_VIEWS.indexOf(current)
  return MD_VIEWS[(at + 1) % MD_VIEWS.length] ?? 'text'
}

/* --- the divider ---------------------------------------------------------------------------- */

/** How narrow either half of a split may get, as a fraction of the pane. */
export const MIN_RATIO = 0.2
export const MAX_RATIO = 0.8
export const DEFAULT_RATIO = 0.5

export function clampRatio(ratio: number): number {
  if (!Number.isFinite(ratio)) return DEFAULT_RATIO
  return Math.min(MAX_RATIO, Math.max(MIN_RATIO, ratio))
}

let ratio = DEFAULT_RATIO
const listeners = new Set<() => void>()

/**
 * Where the divider sits, as the buffer's share of the pane.
 *
 * **Session-global and module-level**, in the shape `highlightLevel.ts` uses: one number every
 * markdown pane in this window shares, held until the window closes and never written to disk.
 *
 * Not persisted, and that is a decision rather than an omission. Persisting it would mean a
 * second field on the wire — `ViewPosition` is per file, and a per-file divider position is a
 * thing nobody has asked for — or a settings write per drag, which is the cost
 * `crates/cide-ipc/src/positions.rs` explains at length that a gesture must not pay. A divider
 * that starts in the middle of every session is the behaviour every split view in this app has.
 */
export function splitRatio(): number {
  return ratio
}

export function setSplitRatio(next: number): void {
  const clamped = clampRatio(next)
  if (clamped === ratio) return
  ratio = clamped
  for (const listener of listeners) listener()
}

export function subscribeSplitRatio(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

/** Testing seam, and what a fresh window would see. Never called by the app. */
export function resetSplitRatioForTest(): void {
  ratio = DEFAULT_RATIO
}
