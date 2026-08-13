/**
 * Which rows a gesture selects — the git tree's *third* concept, and the one it did not have.
 *
 * > *"git changes tree - i should be able to drag one or multiple selected elements
 * > (including whole directories) to another changelist"*
 *
 * # Three concepts, and why none of them can be folded into another
 *
 * | | what it means | where it lives |
 * | --- | --- | --- |
 * | **tick** | what a commit will take | `useGitPanel::selected`, `model.ts::toggleRow` |
 * | **current row** | what the arrows walk and what Space acts through | `useGitPanel::current` |
 * | **row selection** | what a gesture is *about* — this file | `useGitPanel::selection` |
 *
 * Ticking is a statement about a commit. Selecting is a statement about what you are pointing
 * at. They were conflated for one release and the symptom was reported as a drag bug: `grab`
 * widened by *ticks*, and the active changelist opens fully ticked (`model.ts::defaultSelection`),
 * so dragging one file out of it silently moved every file in it. Nothing in the gesture said
 * so until the ghost appeared, by which point the press had happened. A real row selection is
 * the fix for that as much as it is the feature — a drag now carries what the user selected,
 * which is a set they built and can see.
 *
 * `dragDrop.ts` used to record the decision *not* to build this ("the alternative that lost:
 * IDEA's ctrl-click row multi-selection … this panel has no such selection, and inventing one
 * changes what every click in the tree means"). That reasoning was sound and it is now
 * superseded: the click meanings are all here, in one place, executed by `check:git`.
 *
 * # Pure and import-free apart from `type Row`
 *
 * Same arrangement as `model.ts`, `dragDrop.ts` and `clickSemantics.ts`, and for the same
 * reason: there is no JS test runner in this project, so `ui/scripts/check-git-tree.mjs`
 * compiles this module with a bare `tsc` and imports the output under node. The import above
 * is type-only, so the emitted `.js` has no imports at all. Do not add a value import.
 *
 * # What is selectable, and what an anchor is
 *
 * File rows and directory rows. Both are drag sources (`dragDrop.ts::grab`) and both name a
 * set of files, which is what every gesture downstream of a selection wants. Repository and
 * changelist rows are **not** selectable: a press on one folds it, they are not drag sources,
 * and a shift-range that swept a changelist header in would carry the whole list — the exact
 * surprise this feature exists to remove.
 *
 * The **anchor** is a position, not a member. It is where a shift-range starts, so it moves on
 * every plain press and every unmodified arrow and stays put while shift extends. It may name
 * a row that is not itself selectable (a header the user arrowed through), because the range
 * is filtered to selectable rows on the way out — that is what lets shift+↓ walk across a
 * changelist boundary without pulling the boundary in.
 */
import type { Row } from './model'

/** The row selection, and where a shift-range would start from. */
export interface RowSelection {
  readonly ids: ReadonlySet<string>
  /** A row id, or `null` before the tree has been touched. Not necessarily in `ids`. */
  readonly anchor: string | null
}

/** Nothing selected. Exported so the panel's initial state has a name rather than a literal. */
export const NO_ROWS: RowSelection = { ids: new Set<string>(), anchor: null }

/** The two modifiers that change what a press or an arrow means. */
export interface SelectMods {
  /** Ctrl on Linux and Windows, ⌘ on macOS — the caller folds `metaKey` into this. */
  readonly ctrl: boolean
  readonly shift: boolean
}

/**
 * What a press does to the selection, and whether it has to wait for the release.
 *
 * `deferred` is the detail that makes dragging a multi-row selection possible at all. A plain
 * press on a row that is *already* part of a multi-row selection must not collapse the
 * selection to that row on mousedown, because the press is also how a drag of the whole
 * selection begins — collapsing first would destroy the set before the pointer had moved a
 * pixel, and "drag the four files I selected" would be unreachable. So the collapse waits for
 * the mouseup and is skipped when the gesture turned out to be a drag. `ChangesTree` already
 * holds the signal it needs for that (`drag.dragged()`), because folding a group is deferred
 * for exactly the same reason.
 */
export interface PressPlan {
  readonly next: RowSelection
  readonly deferred: boolean
}

/** A row a selection may contain. Repo and changelist rows are positions, not members. */
export function isSelectable(row: Row): boolean {
  return row.kind === 'file' || row.kind === 'dir'
}

/**
 * A left press on `id`, with modifiers.
 *
 * * **plain, on an unselected row** — that row alone. This is what makes "a drag starting on
 *   an unselected row selects it first" true without the drag knowing anything about
 *   selection: the press has already run by the time the 4px threshold is crossed.
 * * **plain, on a row already in a multi-row selection** — nothing yet; see `PressPlan`.
 * * **plain, on the only selected row** — the same thing again, so it is not deferred: there
 *   is no set to preserve and deferring would leave the anchor stale.
 * * **ctrl** — toggle that row and move the anchor to it. Never opens a diff and never folds
 *   a group; the caller gates those on an unmodified press.
 * * **shift** — the inclusive range from the anchor, filtered to selectable rows, with the
 *   anchor left where it was so a second shift-click re-extends from the same end rather than
 *   from the last one. With no anchor yet, shift behaves as a plain press.
 * * **a row that is not selectable** — the selection empties and the anchor moves there. It is
 *   the same rule as an unmodified arrow onto a changelist header, and the alternative
 *   (leaving the selection alone) makes the header the one row in the tree you cannot click to
 *   clear a selection you no longer want.
 */
export function pressSelect(
  rows: readonly Row[],
  sel: RowSelection,
  id: string,
  mods: SelectMods,
): PressPlan {
  if (mods.shift && sel.anchor !== null) {
    const ids = between(rows, sel.anchor, id)
    // A `null` range means one of the two ends is no longer a row — its group was folded away
    // under a background refresh, say. Falling back to the plain rule beats extending from a
    // row that is not on screen, which would select a band the user cannot see the top of.
    if (ids !== null) return { next: { ids, anchor: sel.anchor }, deferred: false }
  }
  if (mods.ctrl) {
    if (!selectable(rows, id)) return { next: { ids: sel.ids, anchor: id }, deferred: false }
    const ids = new Set(sel.ids)
    if (!ids.delete(id)) ids.add(id)
    return { next: { ids, anchor: id }, deferred: false }
  }
  if (!selectable(rows, id)) return { next: { ids: new Set<string>(), anchor: id }, deferred: false }
  if (sel.ids.has(id) && sel.ids.size > 1) return { next: sel, deferred: true }
  return { next: only(id), deferred: false }
}

/**
 * The release of a deferred press: collapse to the row the press landed on.
 *
 * Called only when `pressSelect` said `deferred` *and* the gesture did not become a drag.
 */
export function releaseSelect(rows: readonly Row[], sel: RowSelection, id: string): RowSelection {
  if (!selectable(rows, id)) return sel
  return only(id)
}

/**
 * An arrow, Home or End that has already decided which row it is moving to.
 *
 * The cursor is the caller's business — it always moves. This answers the other half.
 *
 * * **shift** extends from the anchor, exactly as shift-click does, so shift+↓↓↓ and
 *   shift-clicking three rows down produce the same set.
 * * **ctrl** moves the cursor and leaves the selection completely alone, which is how a
 *   keyboard user reaches a row to ctrl-Space or ctrl-click without losing what they have.
 * * **neither** takes the selection along, which is what every tree does.
 */
export function keySelect(
  rows: readonly Row[],
  sel: RowSelection,
  to: string,
  mods: SelectMods,
): RowSelection {
  if (mods.shift && sel.anchor !== null) {
    const ids = between(rows, sel.anchor, to)
    if (ids !== null) return { ids, anchor: sel.anchor }
  }
  if (mods.ctrl && !mods.shift) return sel
  if (!selectable(rows, to)) return { ids: new Set<string>(), anchor: to }
  return only(to)
}

/**
 * Ctrl+A: every selectable row that is on screen.
 *
 * The *visible* rows, deliberately — `rows` holds nothing for a collapsed changelist. "Select
 * all" that reached into folded groups would hand a drag a set whose size the user cannot see,
 * which is the trap the tick-widening had. Fold a group open and press it again.
 */
export function selectAll(rows: readonly Row[]): RowSelection {
  const ids = new Set<string>()
  let anchor: string | null = null
  for (const row of rows) {
    if (!isSelectable(row)) continue
    if (anchor === null) anchor = row.id
    ids.add(row.id)
  }
  return { ids, anchor }
}

/** Escape: back to one row, the one the cursor is on. `null` clears outright. */
export function collapseTo(id: string | null): RowSelection {
  return id === null ? NO_ROWS : only(id)
}

/**
 * Drop selected ids whose row no longer exists.
 *
 * Called on every authoritative refresh, beside the tick set's own pruning, and for the same
 * reason: a set that only ever grows would eventually name paths from a repository that has
 * been closed. `alive` is file row ids plus every expandable row id (`model.ts::allGroups`
 * already returns directory rows), so a selection survives a group being folded — collapsing
 * a changelist hides rows, it does not deselect them.
 *
 * The anchor is kept whatever happens to it. It is a position rather than a member, a stale
 * one simply makes the next shift-range fall back to the plain rule, and clearing it would
 * lose the range's origin every time an agent touched a file somewhere else in the tree.
 */
export function pruneRowSelection(alive: ReadonlySet<string>, sel: RowSelection): RowSelection {
  const ids = new Set<string>()
  for (const id of sel.ids) if (alive.has(id)) ids.add(id)
  return ids.size === sel.ids.size ? sel : { ids, anchor: sel.anchor }
}

/**
 * The selection as `dragDrop.ts::grab` wants it: every selected id, plus every file row id
 * under a selected *directory*.
 *
 * Two membership questions are asked of the result and it has to answer both — "is the row
 * this gesture started on part of the selection" (which can be a directory) and "is this file
 * one the selection carries" — so it holds directory ids as well as file ids. The two kinds
 * can never collide: `fileRowId` is `repo\0f\0path` and `dirRowId` is `repo\0d\0group\0path`.
 *
 * Resolved here rather than inside `grab` because resolving a directory needs the *row* (its
 * `files`), and `grab` is given the view. A selected file id passes straight through, so a
 * file inside a folded group is still carried — exactly as a tick is.
 */
export function carriedIds(rows: readonly Row[], sel: RowSelection): Set<string> {
  const out = new Set<string>()
  for (const id of sel.ids) out.add(id)
  for (const row of rows) {
    if (row.kind !== 'dir' || !sel.ids.has(row.id)) continue
    for (const id of row.files) out.add(id)
  }
  return out
}

/** One row, and the anchor on it. */
function only(id: string): RowSelection {
  return { ids: new Set([id]), anchor: id }
}

function selectable(rows: readonly Row[], id: string): boolean {
  const row = rows.find((r) => r.id === id)
  return row !== undefined && isSelectable(row)
}

/**
 * The inclusive band between two rows, in row order, keeping only what may be selected.
 *
 * `null` when either end is not a row — the caller falls back to the unmodified rule rather
 * than guessing. Walks the flat `rows` array by index, which is already how the tree finds the
 * current row; there is no tree structure to descend because there is no nested markup.
 */
function between(rows: readonly Row[], from: string, to: string): Set<string> | null {
  const a = rows.findIndex((row) => row.id === from)
  const b = rows.findIndex((row) => row.id === to)
  if (a < 0 || b < 0) return null
  const lo = Math.min(a, b)
  const hi = Math.max(a, b)
  const ids = new Set<string>()
  for (let i = lo; i <= hi; i++) {
    const row = rows[i]
    if (row !== undefined && isSelectable(row)) ids.add(row.id)
  }
  return ids
}
