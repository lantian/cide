/**
 * Which rows a gesture selects in the **file tree** — the sibling of `GitPanel/rowSelection.ts`.
 *
 * > *"file tree - can not select multiple elements with CTRL + mouse and SHIFT + mouse
 * > (like we've implemented for git tree)"*
 *
 * The git panel grew a real row selection first and the rules there are the reference: a plain
 * press collapses to one row, Ctrl toggles one, Shift takes the inclusive band from the anchor,
 * and the anchor is a *position* that Shift leaves alone so a second Shift-click re-extends from
 * the same end. Those rules are repeated here rather than shared, and the next two sections are
 * why — the two trees agree on what the gestures mean and disagree on everything underneath.
 *
 * # A selection of paths, not of rows
 *
 * `ChangesTree` has every row in memory, so its selection can be a set of row ids and a range
 * can be a scan of the array. This tree is windowed: `treeStore` holds a bounded number of
 * chunks and a 100k-file repository has 100k rows, so there is no array to scan and a row id
 * would name a position that the next watcher burst renumbers. The selection is therefore a set
 * of **paths**, for exactly the reason `treeStore.selected` has always been one — a path
 * survives a file being created above it, and it is also what every action downstream wants.
 *
 * The cost is that this module cannot resolve a Shift-range by itself. It says *that* a range
 * is wanted ([`SelectPlan`]) and the store, which can reach Rust, says which paths are in it and
 * hands them back to [`applyRange`]. The rule stays here where a check script can hold it; only
 * the fetch is out there.
 *
 * # Why there is no pruning
 *
 * `rowSelection.ts` drops selected ids whose row is gone on every authoritative refresh. The
 * same call here would be a bug: `treeStore.indexOf` searches *resident* chunks only, so "not
 * found" means "not on screen and not nearby" — the ordinary state of a selected row after a
 * scroll — and pruning against it would empty the selection as the user scrolled away from it.
 * There is no cheap way to ask "does this path still have a row" for an arbitrary path
 * (`fs_reveal` answers it, and expands every ancestor as a side effect). So the selection is
 * cleared by the things that genuinely invalidate it — a project switch, and the delete that
 * consumed it — and a path that goes stale in between is refused by Rust, which is the one
 * place that actually knows.
 *
 * # Pure and import-free
 *
 * The same arrangement as `clickSemantics.ts`, `rowPaths.ts` and `newEntry.ts`: there is no JS
 * test runner in this project, so `ui/scripts/check-tree-select.mjs` compiles this one module
 * with a bare `tsc` and imports the output under node. Do not add an import.
 */

/** The selected rows, and where a Shift-range would start from. */
export interface TreeSelection {
  /**
   * Absolute paths. Iteration order is the order they were picked, which is tree order for a
   * band and click order for a sequence of Ctrl-clicks — the order `FileClip.paths` documents.
   */
  readonly paths: ReadonlySet<string>
  /** A path, or `null` before the tree has been touched. Not necessarily in `paths`. */
  readonly anchor: string | null
}

/** Nothing selected. Exported so the store's initial state has a name rather than a literal. */
export const NO_PATHS: TreeSelection = { paths: new Set<string>(), anchor: null }

/** The two modifiers that change what a press or an arrow means. */
export interface SelectMods {
  /** Ctrl on Linux and Windows, ⌘ on macOS — the caller folds `metaKey` into this. */
  readonly ctrl: boolean
  readonly shift: boolean
}

/** Neither modifier: what an ordinary click and an unmodified arrow carry. */
export const NO_MODS: SelectMods = { ctrl: false, shift: false }

/**
 * What a gesture asks for: a selection this module could work out on its own, or a band it
 * cannot see.
 *
 * The `range` case names its two ends by path and nothing else. Turning those into a set of
 * paths needs row *indices*, which only `treeStore` has, and may need a fetch — see the module
 * header. The store answers it by calling [`applyRange`].
 */
export type SelectPlan =
  | { readonly kind: 'set'; readonly next: TreeSelection }
  | { readonly kind: 'range'; readonly from: string; readonly to: string }

/**
 * How many rows one Shift-range or Ctrl+A may cover.
 *
 * A cap and not a convenience. The band is resolved by asking Rust for those rows, so an
 * unbounded range on a 100k-file repository is a 100k-row IPC payload assembled on the GTK main
 * loop — and what it would buy is a *destructive* menu ("Move 100 000 files to Trash") whose
 * scope the user cannot check. 10 000 is far above any selection anyone assembles deliberately
 * and far below the size at which either of those becomes a problem.
 *
 * Refused out loud rather than silently truncated ([`rangeRefusal`]): a highlight that stops
 * somewhere the user did not ask it to is indistinguishable from a range that went wrong.
 */
export const RANGE_ROWS = 10_000

/** Why this band cannot be selected, or `null` when it can. A sentence, for the problem strip. */
export function rangeRefusal(rows: number): string | null {
  if (rows <= RANGE_ROWS) return null
  return `That range covers ${rows} rows; at most ${RANGE_ROWS} can be selected at once.`
}

/**
 * A left press on `path`, with modifiers.
 *
 * * **plain** — that row alone, and the anchor moves to it.
 * * **ctrl** — toggle that row, and the anchor moves to it. The caller must not also open or
 *   fold anything: Ctrl-click means "add this row", and a folder that folded while a selection
 *   was being assembled would renumber every row below it mid-gesture.
 * * **shift** — the inclusive band from the anchor, with the anchor left where it was so a
 *   second Shift-click re-extends from the same end rather than from the last one. With no
 *   anchor yet it is a plain press.
 *
 * Shift is tested before Ctrl, so Ctrl+Shift is Shift — the same order `rowSelection.ts` uses.
 * IDEA adds the band to the existing selection there; matching the tree next door matters more
 * than matching IDEA, because these two panels sit one icon apart in the same rail.
 *
 * Unlike the git tree there is no unselectable row: every row in this tree is a file or a
 * folder, both of which are things the menu can act on.
 */
export function pressSelect(sel: TreeSelection, path: string, mods: SelectMods): SelectPlan {
  if (mods.shift && sel.anchor !== null) return { kind: 'range', from: sel.anchor, to: path }
  if (mods.ctrl) {
    const paths = new Set(sel.paths)
    if (!paths.delete(path)) paths.add(path)
    return { kind: 'set', next: { paths, anchor: path } }
  }
  return { kind: 'set', next: only(path) }
}

/**
 * An arrow, Home or End that has already decided which row it is moving to.
 *
 * The cursor is the caller's business — it always moves. This answers the other half.
 *
 * * **shift** extends from the anchor, exactly as Shift-click does, so Shift+↓↓↓ and
 *   Shift-clicking three rows down produce the same set.
 * * **ctrl** moves the cursor and leaves the selection completely alone, which is how a
 *   keyboard user reaches a row to Ctrl-click without losing what they have.
 * * **neither** takes the selection along, which is what every tree does.
 */
export function keySelect(sel: TreeSelection, to: string, mods: SelectMods): SelectPlan {
  if (mods.shift && sel.anchor !== null) return { kind: 'range', from: sel.anchor, to }
  if (mods.ctrl) return { kind: 'set', next: sel }
  return { kind: 'set', next: only(to) }
}

/**
 * The answer to a `range` plan: the band the store resolved, or the plain rule when it could
 * not resolve one.
 *
 * `null` is not an error state, it is the fallback, and it has two ordinary causes: the anchor
 * is inside a folder that has since been collapsed, so it has no row at all; or it has been
 * scrolled far enough out of the windowed cache that its index is unknown, and `fs_reveal` —
 * the only call that could answer — expands ancestors as a side effect. Extending from a row
 * whose position is a guess would select a band the user cannot see the top of, so the gesture
 * degrades to what an unmodified click would have done. The anchor moves with it, which is what
 * makes the *next* Shift-click work from somewhere the user can see.
 */
export function applyRange(
  sel: TreeSelection,
  band: readonly string[] | null,
  to: string,
): TreeSelection {
  if (band === null || band.length === 0) return only(to)
  // The anchor is deliberately kept: a band is an extension *from* somewhere, and moving the
  // anchor to the far end would make a second Shift-click extend from the wrong side.
  return { paths: new Set(band), anchor: sel.anchor ?? to }
}

/** Escape: back to the one row the cursor is on. `null` clears outright. */
export function collapseTo(path: string | null): TreeSelection {
  return path === null ? NO_PATHS : only(path)
}

/**
 * What a **right-click** on `path` selects.
 *
 * A menu opened on a row that is already part of the selection leaves it alone; anywhere else
 * collapses to the row that was clicked. This is the rule that makes a multi-row *Move to
 * Trash* reachable at all — without it the right-click that opens the menu would first destroy
 * the selection the menu is supposed to act on — and it is also the rule that stops a menu from
 * acting on rows the user is no longer pointing at.
 *
 * The result doubles as the menu's scope: [`actionScope`] over it names exactly the rows the
 * tree is showing as selected at the moment the menu opens, which is the thing the user can
 * check before clicking.
 */
export function pressMenu(sel: TreeSelection, path: string): TreeSelection {
  return sel.paths.has(path) ? sel : only(path)
}

/**
 * The rows a Cut, Copy or Delete acts on: exactly what is highlighted, in pick order.
 *
 * Deliberately **not** falling back to the cursor when the selection is empty. The two can
 * differ by one Ctrl-click — clicking the last selected row off leaves the cursor on a row that
 * is no longer highlighted — and a Delete that took the cursor there would move a file to the
 * trash with nothing on screen saying it was the target. An empty answer means the keystroke is
 * not ours, which is the same thing "nothing is selected" has always meant in this panel.
 */
export function actionScope(sel: TreeSelection): readonly string[] {
  return [...sel.paths]
}

/** One row, and the anchor on it. */
function only(path: string): TreeSelection {
  return { paths: new Set([path]), anchor: path }
}
