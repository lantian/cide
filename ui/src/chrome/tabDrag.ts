/**
 * Dragging a tab along the strip: what a grab picks up, where the caret goes, and what a drop
 * actually commits.
 *
 * The sibling of `sidebar/treeDrag.ts` and `sidebar/GitPanel/dragDrop.ts`, and deliberately the
 * same shape: every *decision* is a plain function over plain data — no React, no DOM, no IPC —
 * so `ui/scripts/check-tab-drag.mjs` can run it under node. The *mechanism* (the pointer state
 * machine, the 4px threshold, the hit testing) is `useTabDrag.ts`, and nothing in this repo can
 * execute that. This project has paid six times for a rule that lived inside an event handler,
 * and the arithmetic below has an off-by-one in it that is invisible in a screenshot.
 *
 * **Import only types, and only from import-free modules.** `check-tab-drag.mjs` runs a bare
 * `tsc` over this file; a value import — React, a CSS module, `@/ipc/client` — is how that stops
 * working. The constraint `menuModel.ts` states about itself, for the same reason.
 *
 * # The pinned console, expressed three ways so a user never meets the refusal
 *
 * `cide_core::workspace::reorder_tab` refuses two things: moving `tabs[0]`, and moving anything
 * *to* index 0. Both are enforcement and stay that way. But a drag that bounces back is worse
 * than a drag that never starts, so the rules here make both unreachable:
 *
 * 1. [`grabTab`] returns `null` for the console, so the press never becomes a gesture.
 * 2. [`caretIndex`] **clamps** to boundary 1 rather than hiding the caret. Clamping is the
 *    honest version: it shows where the drop will actually land. A hidden caret over the left
 *    half of the strip looks exactly like a drag the app has stopped tracking, which is the
 *    failure this repository keeps shipping.
 * 3. [`dropOutcome`] can therefore never produce a boundary of 0.
 *
 * Read off `kind === 'claudeHome'` and never off the index, for the reason `TabStrip.tsx` and
 * `menuModel.closable` both state: array order is a rendering detail, whereas the pin is a
 * property of the tab. That matters more here than anywhere else in the app, because reordering
 * is the one gesture whose entire purpose is to make the index lie.
 */

/**
 * The parts of a `Tab` a drag reads.
 *
 * Structural, so the generated `Tab` satisfies it and this module imports nothing from the wire
 * types — the same arrangement `treeDrag.DragRow` and `menuModel.RecentLike` use. TypeScript
 * checks the real one against it at the call site in `useTabDrag.ts`.
 */
export interface DragTab {
  readonly id: string
  readonly kind: { readonly kind: string }
}

/** Whether this tab may be *moved*. The pinned console may not. */
export function movable(tab: DragTab): boolean {
  return tab.kind.kind !== 'claudeHome'
}

/**
 * The first boundary a tab may be dropped at.
 *
 * One, not zero, because index 0 is the pinned console and `reorder_tab` refuses a `to` of 0.
 * Named rather than written as a literal `1` at each of the three sites that need it: it is the
 * same fact three times, and three literals is how two of them stay right and one drifts.
 */
export const FIRST_BOUNDARY = 1

/** What a press picked up, or `null` when that tab is not a drag source. */
export interface TabDragSet {
  /** The dragged tab's id — what `tab.reorder` is given as `tab`. */
  readonly tab: string
  /** Where it started, so a drop that lands back there can be recognised as a no-op. */
  readonly from: number
}

/**
 * What a press on `tab` picks up, or `null` when that tab cannot be dragged at all.
 *
 * `null` for the pinned console and for a tab that is not in this strip. Unlike `treeDrag.grab`,
 * there is no "picked up but refused" state carrying a sentence: the tree has that because a
 * project root is a real file the user grabbed and deserves an answer, whereas the console is a
 * tab whose entire visible contract is that it does not move — it draws no close button either.
 * A console that lifted off the strip and refused every destination would be teaching a rule
 * nobody asked about.
 */
export function grabTab(tabs: readonly DragTab[], tab: DragTab): TabDragSet | null {
  if (!movable(tab)) return null
  const from = tabs.findIndex((t) => t.id === tab.id)
  if (from < 0) return null
  return { tab: tab.id, from }
}

/**
 * Which boundary the pointer is aiming at: the gap *before* the returned index.
 *
 * Boundaries, not tabs, because that is what a drop means and what the caret draws. With four
 * tabs there are five boundaries, 0 through 4, and 0 is unreachable by the clamp above.
 *
 * `overIndex` is the tab under the pointer and `fraction` is how far across it the pointer sits,
 * 0 at its left edge and 1 at its right. Past the midpoint means "after this tab", which is the
 * boundary one to its right. A midpoint rather than a fixed pixel inset because tabs here differ
 * in width by a factor of three — `Claude` against a long filename — and a fixed inset would put
 * the flip point outside a narrow tab entirely.
 *
 * `overIndex` of `-1` — the pointer is on the strip but on no tab, which is the spacer to the
 * right of the last tab — means the end. The spacer is most of the strip on a project with two
 * tabs open, so this is the common case rather than an edge one.
 */
export function caretIndex(count: number, overIndex: number, fraction: number): number {
  if (overIndex < 0) return count
  const boundary = fraction >= 0.5 ? overIndex + 1 : overIndex
  // Clamped, never hidden. See the module header: a caret that vanishes over the left of the
  // strip is indistinguishable from a drag that has stopped working.
  return Math.min(Math.max(boundary, FIRST_BOUNDARY), count)
}

/**
 * What a drop at `boundary` commits, or `null` when it would change nothing.
 *
 * `before` is the id of the tab the dragged one lands in front of, or `null` for the end of the
 * strip — the shape `tab.reorder` takes, and the reason the command takes ids: this value is
 * computed against a snapshot and sent over IPC, and the strip can change in between.
 *
 * **The two no-ops are the whole subtlety, and one of them is not obvious.** Dropping at the
 * boundary immediately *before* the dragged tab leaves it where it is — obviously. Dropping at
 * the boundary immediately *after* it does too, because removing the tab closes the gap the
 * boundary was addressing. Sending either to Rust would be a round trip, a `rev` bump and a
 * broadcast to every window to achieve nothing; worse, it would make an accidental 5px twitch
 * on a tab indistinguishable in the workspace file from a deliberate reorder.
 */
export function dropOutcome(
  tabs: readonly DragTab[],
  drag: TabDragSet,
  boundary: number,
): { readonly tab: string; readonly before: string | null } | null {
  if (boundary === drag.from || boundary === drag.from + 1) return null
  if (boundary < FIRST_BOUNDARY || boundary > tabs.length) return null
  const before = tabs[boundary]
  return { tab: drag.tab, before: before === undefined ? null : before.id }
}
