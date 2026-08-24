/**
 * Which surface a *next change* / *previous change* command acts on. (M25)
 *
 * `keys/dispatch.ts` fires `navigate.nextChange` with no pane, no DOM node and no way to get
 * one — the same predicament `editor/caretTrack.ts` exists for — so the surface that can answer
 * announces itself here and the dispatcher asks. Two surfaces claim: `panes/GitDiffPane.tsx`'s
 * `GitDiffView` (which serves the git panel's diff tab, the log tool window's revision diff and
 * the smoke fixture) and `panes/MergePane.tsx`, whose block stepper predates this module and is
 * routed through it rather than duplicated.
 *
 * # Why this is a second stack and not a field on the caret slot
 *
 * `caretTrack.ts` argues — correctly, and at length above `FoldActions` — that folding rides the
 * caret slot because folding answers the question that module already answers: *which editor is
 * the user in*. That argument does not reach here, and following it anyway would break things.
 * A diff is not an editor: it has no caret, no path in the sense `CaretPosition` means, and no
 * `EditorView` at all in the git pane's case. Claiming the caret slot from a diff would make
 * `focusedCaret()` answer with a fabricated line and column, and Go to line, ⌥F7, the member walk
 * and Find usages would all start acting on a document that is not open.
 *
 * The question here is a genuinely different one — *which change-walkable surface is in front* —
 * and there is exactly **one** stack for it, shared by both surfaces, which is the rule
 * `caretTrack`'s header actually states. A third surface (a future `DiffPane` chunk walk) joins
 * this one rather than opening a third.
 *
 * # Two departures from `caretTrack`, both borrowed from `editor/statusReadout.ts`
 *
 * `onScreen`, because `TabContent` never unmounts an inactive tab: five open diff tabs all hold
 * live claims, and an unconditional push would leave the slot with whichever mounted *last*
 * rather than whichever the user is looking at. That is the restore bug `statusReadout.ts`
 * records in full, and the fix is the same word — a mount behind another tab goes to the bottom
 * of the stack rather than being skipped, so closing the visible one hands the slot down instead
 * of blanking it.
 *
 * And [`subscribeChangeNav`], because `App.tsx` publishes `diffFocused` as a key-context flag and
 * has to re-render when the answer changes.
 *
 * # Deliberately import-free
 *
 * `scripts/check-diff-render.mjs` compiles this standalone and drives [`stepIndex`] directly,
 * which is the half worth pinning: the clamp is shared by both surfaces precisely so they cannot
 * disagree about what happens at the ends, and a shared rule that nothing measures is two rules
 * waiting to diverge.
 */

/** Where a walk is. A plain pair, so a caller can render "3 of 12" without a second question. */
export interface ChangeCursor {
  /** The current change, or `-1` when the surface has not been walked yet. */
  readonly index: number
  readonly count: number
}

/**
 * What a surface that can walk changes offers.
 *
 * `step` answers **whether it did anything**, so a chord that finds nowhere to go can say so
 * through `unmet(...)` rather than being a keystroke that silently did nothing. That is
 * `FoldActions`' rule, and it exists for the failure `cide_core::commands`' header is written
 * against: a command that is listed, bound, and inert.
 */
export interface ChangeNav {
  step(delta: 1 | -1): boolean
  cursor(): ChangeCursor
}

/** The handle a surface holds. Inert after `release`. */
export interface ChangeNavSlot {
  /** Take the slot — this surface became the one in front. */
  focus(): void
  release(): void
}

/** Nothing is current and there is nothing to walk. */
export const NO_CURSOR: ChangeCursor = { index: -1, count: 0 }

/**
 * Where a step lands, or `null` when it lands nowhere.
 *
 * **Clamps, never wraps**, and both surfaces share this function so that they cannot come to
 * disagree about it. `editor/memberNav.memberStep` made the same choice for the same first
 * reason — a held key must stop at the end rather than teleport to the other one — and two more
 * apply here. A refusal is *reportable*: `unmet(command, 'already at the last change')` puts a
 * line in the log, where a wrap on a one-change diff is indistinguishable from a chord that did
 * nothing at all. And the ends being reachable is what lets the toolbar buttons draw themselves
 * disabled, which states the same fact where the user is already looking.
 *
 * `current < 0` — nothing walked yet — lands on the **first** change in either direction. Not the
 * last for a backwards step: the reader is at the top of a file they have just opened, and
 * *Previous change* from there meaning "jump to the bottom" is the wrap this function refuses,
 * spelled differently.
 */
export function stepIndex(count: number, current: number, delta: 1 | -1): number | null {
  if (count <= 0) return null
  if (current < 0) return 0
  const next = current + delta
  return next < 0 || next >= count ? null : next
}

/**
 * Newest last, so the top of the stack is the surface the user is in.
 *
 * A plain array rather than a `Map`, for `caretTrack`'s reason: two panes can show the same file,
 * so no property of the surface is a key. Identity is the claim object itself, which is what
 * `release` removes.
 */
const claims: ChangeNav[] = []
const listeners = new Set<() => void>()

function publish(): void {
  for (const listen of [...listeners]) listen()
}

/** The surface a change command should act on, or `null` when nothing can answer. */
export function focusedChangeNav(): ChangeNav | null {
  return claims.at(-1) ?? null
}

/**
 * Whether anything holds the slot.
 *
 * A **boolean**, and deliberately not `focusedChangeNav()` or a `ChangeCursor`. `App.tsx` reads
 * this through `useSyncExternalStore`, which compares snapshots with `Object.is`: a getter
 * returning a fresh object would never compare equal, and the render loop that follows ends at
 * *Maximum update depth exceeded* with the whole root unmounted. That is the failure
 * `scripts/check-selectors.mjs` exists for, arriving through a different hook — so the shape is
 * fixed here, where the comment can sit beside the value.
 */
export function changeNavPresent(): boolean {
  return claims.length > 0
}

/**
 * Announce a surface that can walk changes.
 *
 * `nav` must have **stable identity** for the life of the claim: it is what `release` removes,
 * and a fresh object per render would leave a stack full of corpses. The two call sites both
 * delegate through a ref for that reason — the same idiom `GitDiffPane`'s `marksRef.current =
 * marks` already uses.
 *
 * `onScreen` is whether this surface's tab is the one in front; see the header.
 */
export function claimChangeNav(nav: ChangeNav, onScreen = true): ChangeNavSlot {
  if (onScreen) claims.push(nav)
  else claims.unshift(nav)
  publish()
  let live = true

  return {
    focus(): void {
      if (!live || claims.at(-1) === nav) return
      const at = claims.indexOf(nav)
      if (at < 0) return
      claims.splice(at, 1)
      claims.push(nav)
      publish()
    },
    release(): void {
      if (!live) return
      live = false
      const at = claims.indexOf(nav)
      if (at >= 0) claims.splice(at, 1)
      publish()
    },
  }
}

/**
 * Watch whether anything holds the slot.
 *
 * Deliberately does **not** fire immediately, unlike `subscribeStatusReadout`: the one caller is
 * `useSyncExternalStore`, which reads the snapshot itself and only wants to be told about
 * changes. An immediate call would be a wasted render on every mount.
 */
export function subscribeChangeNav(listen: () => void): () => void {
  listeners.add(listen)
  return () => {
    listeners.delete(listen)
  }
}

/** Testing seam. Never called by the app. */
export function resetChangeNavForTest(): void {
  claims.length = 0
  listeners.clear()
}
