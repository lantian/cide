/**
 * Which gesture on which row, in which state, produces which action — for all three sidebar
 * trees.
 *
 * Four user reports produced these rules and every one of them is about a *decision*, not
 * about markup: a click that opened a file when it should only have selected it, a click in
 * the git tree that opened a diff when a diff was not even on screen, a search hit that
 * opened a file at the wrong line. None of that is visible in a screenshot and none of it is
 * reachable from a check script if it lives inside three `onClick` handlers, so it lives
 * here and `check-tree-status.mjs`, `check-search.mjs` and `check-git-tree.mjs` each pin the
 * rule belonging to their tree.
 *
 * Pure and **import-free**, the same arrangement as `treeStatus.ts`, `SearchModel.ts` and
 * `rowWindow.ts`: there is no JS test runner in this project, so a check script compiles this
 * one module with a bare `tsc` and imports the output under node. Do not add an import.
 *
 * # Why there is no timer anywhere in this file
 *
 * The obvious way to tell a single click from a double is to wait ~250 ms after the first one
 * to see whether a second arrives. It is also the wrong way: it makes *every* single click
 * feel late, which in a file tree is every click the user makes, and it buys nothing —
 * `MouseEvent.detail` already counts the clicks in the current sequence. The browser tells us
 * for free what a timer would spend a quarter of a second guessing.
 *
 * That leaves one hazard, which is the whole reason [`gestureOf`] exists: on a double-click
 * the `click` event fires **twice**, with `detail` 1 and then 2. So the single-click action
 * runs, and then the double-click action runs. Every rule below is written with that in mind
 * — the `single` action is always something that is harmless to have done first (moving the
 * selection to the row you are about to open), and no rule performs a toggle on both halves
 * of one gesture, which would expand a folder and immediately collapse it again.
 */

/** One click, or the second click of a double. See the module header. */
export type Gesture = 'single' | 'double'

/**
 * What a gesture does to the row it landed on.
 *
 * Three independent booleans rather than one enum: "select and open" is two things happening,
 * and an enum would need a member per combination — which is how `selectAndOpen` and
 * `selectAndToggleAndOpen` get invented and then diverge from what the handlers really do.
 */
export interface RowAction {
  /** Move the selection (the file trees) or the cursor (the git tree) to this row. */
  readonly select: boolean
  /** Expand or collapse it. Only ever true for a row that can. */
  readonly toggle: boolean
  /** Open it: a file tab, a diff tab, or a search hit's file at its line. */
  readonly open: boolean
}

const SELECT: RowAction = { select: true, toggle: false, open: false }
const SELECT_OPEN: RowAction = { select: true, toggle: false, open: true }
const SELECT_TOGGLE: RowAction = { select: true, toggle: true, open: false }
/** The second half of a gesture whose first half already did the work. */
const NOTHING: RowAction = { select: false, toggle: false, open: false }

/**
 * `MouseEvent.detail` as a gesture.
 *
 * `>= 2` and not `=== 2`: a triple click reports 3, and the third click of a rapid sequence
 * on a file row should not fall back to "select only" and leave the user's third click
 * looking like it undid the second.
 */
export function gestureOf(detail: number): Gesture {
  return detail >= 2 ? 'double' : 'single'
}

export interface FileTreeClick {
  readonly gesture: Gesture
  /** A directory row expands; a file row opens. */
  readonly isDir: boolean
  /** The pointer was on the ▸/▾ twisty rather than on the row's body. */
  readonly onTwisty: boolean
}

/**
 * The file tree.
 *
 * > *"In file tree when i do one click on element - we should select it, but not open the
 * > file. Open only by double click"*
 *
 * The twisty is deliberately exempt. It is a control whose only meaning is "open this
 * folder", and requiring a double-click on a 11px arrow to do the one thing it exists for
 * would be a worse tree than the one being fixed. It acts on the first click only: a
 * double-click on the arrow is one gesture, and toggling on both halves would expand a
 * folder and collapse it again in the same 200 ms.
 */
export function fileTreeClick({ gesture, isDir, onTwisty }: FileTreeClick): RowAction {
  if (onTwisty) return gesture === 'single' ? SELECT_TOGGLE : NOTHING
  if (gesture === 'single') return SELECT
  return isDir ? SELECT_TOGGLE : SELECT_OPEN
}

export interface GitTreeClick {
  readonly gesture: Gesture
  /** A repository or changelist row. File rows are the leaves. */
  readonly expandable: boolean
  /**
   * A git diff tab is open in this project **right now**.
   *
   * Read from the Rust-owned workspace, not from whether this panel opened one: a diff tab
   * survives a restart, a second window can open one, and the user can close the tab. The
   * sidebar's own memory of what it last opened answers a different question.
   */
  readonly diffOpen: boolean
}

/**
 * The git changes tree.
 *
 * > *"in git files tree when i do one click on element - we should select it, but not open
 * > the diff. Only when diff is already opened one click should change current diff to
 * > selected file. Otherwise diff should be opened only via double click."*
 *
 * The conditional rule is the interesting one and it is the reason this file exists: a single
 * click *switches* a diff that is already on screen and *never opens* one that is not. The
 * user is either reading diffs — in which case clicking down the list is how you read them —
 * or working in the tree, in which case a click that throws a diff tab in front of you is an
 * interruption.
 *
 * Group rows keep the behaviour they had: a single click folds them, because they hold no
 * diff and there is nothing else for a click on one to mean.
 */
export function gitTreeClick({ gesture, expandable, diffOpen }: GitTreeClick): RowAction {
  if (expandable) {
    // The first click already folded it; folding again on the second half of the same
    // gesture would put it straight back.
    return gesture === 'single' ? SELECT_TOGGLE : NOTHING
  }
  if (gesture === 'double') return SELECT_OPEN
  return diffOpen ? SELECT_OPEN : SELECT
}

export interface SearchClick {
  readonly gesture: Gesture
  /** A file heading, or one matching line under it. */
  readonly kind: 'file' | 'hit'
}

/**
 * The search results.
 *
 * > *"search result click doesn't point me to found place (should open file and select the
 * > line)"*
 *
 * A single click opens here, unlike in the two trees, and that is not an inconsistency: a
 * search hit **is** a request to go somewhere. The user typed a query, read a list of places,
 * and picked one. There is nothing else a click on a result could reasonably mean, and every
 * editor treats a results list this way.
 *
 * The second half of a double-click does nothing rather than opening again. Re-opening would
 * be harmless — the tab is already there and the caret is already on the hit — but "one
 * gesture, one navigation" is the invariant that keeps this honest when the open path grows
 * a side effect later.
 */
export function searchClick({ gesture, kind }: SearchClick): RowAction {
  if (kind === 'file') return gesture === 'single' ? SELECT_TOGGLE : NOTHING
  return gesture === 'single' ? SELECT_OPEN : NOTHING
}

/**
 * Enter on the selected row.
 *
 * The keyboard has no ambiguity to resolve — there is no such thing as a double Enter — so
 * this is the one place where "open" is unconditional for a leaf. A directory expands, which
 * is what every tree in every editor does with Enter on a folder.
 */
export function enterOn(row: { readonly expandable: boolean }): RowAction {
  return row.expandable ? SELECT_TOGGLE : SELECT_OPEN
}

/** The modifier state of a keystroke, as the four booleans a `KeyboardEvent` carries. */
export interface KeyMods {
  readonly ctrl: boolean
  readonly meta: boolean
  readonly alt: boolean
  readonly shift: boolean
}

/** What a keystroke asks the file tree to do to the selected row, beyond moving. */
export type TreeKeyAction = 'rename' | 'delete'

/**
 * Rename or delete the selected row, or `null` when the keystroke means neither.
 *
 * # Why these two are decided here and not in `crates/cide-core/src/keymap.rs`
 *
 * Both are **focus-scoped**, not context-scoped, and that is the same argument the tree's
 * clipboard keys already make (see the long comment in `FileTree.tsx`'s `onKeyDown`). Delete
 * in a terminal is a character the pty must receive; Ctrl+R in a shell or a Claude pane is
 * readline's reverse-i-search, which people use constantly. The question a binding would have
 * to answer is "does the file tree hold the caret right now", and the flag that exists —
 * `sidebarFiles` — means the panel is *visible*, which it is while the user types in a
 * terminal beside it. A global binding would be resolved by the key gate's window-capture
 * listener before the event ever reached its target and swallowed in all of those places.
 *
 * A handler on the tree's scroller is asked only when the scroller is focused, which is the
 * actual question. The cost is that neither gets a key chip in the context menu, because chips
 * come from resolving a command id through the keymap and these have no command id — the same
 * trade Cut/Copy/Paste already make one group up in that menu.
 *
 * # Why it is a function in this module rather than a `switch` in the component
 *
 * Because it is a *decision*, and the decisions in this file are the ones a check script can
 * hold. `check-tree-status.mjs` pins every row below. A `case 'Delete':` inside a 1500-line
 * component is reviewable only by reading it, and "Delete with Shift held also deletes" is
 * exactly the kind of thing that is true by accident and invisible in a screenshot.
 *
 * # The rules, and what each one is protecting
 *
 * * **Bare Delete** deletes. Any modifier at all — Ctrl, Alt, Shift, Meta — means *not this*.
 *   Shift+Delete is "delete permanently, no trash" in Windows Explorer and in IDEA, and this
 *   app has no such operation (`fs_delete` always goes to the trash). Answering it with the
 *   ordinary trash delete would silently do something other than what the user asked for, and
 *   the one thing worse than not supporting a destructive gesture is appearing to.
 * * **Ctrl+R** (or ⌘R) renames, with **no** Alt and **no** Shift. `ctrl+shift+r` stays free —
 *   it is "hard reload" muscle memory and belongs to nobody here yet.
 * * Everything else is `null`, so the caller leaves the keystroke alone. Notably `F2`: it is
 *   the rename key in Explorer and IDEA and it is deliberately **not** claimed here, because
 *   the brief asked for Ctrl+R and a second undocumented binding for the same action is a
 *   thing to decide rather than to slip in.
 */
export function treeKeyAction(key: string, mods: KeyMods): TreeKeyAction | null {
  const primary = mods.ctrl || mods.meta
  if (key === 'Delete') {
    return primary || mods.alt || mods.shift ? null : 'delete'
  }
  if (key.toLowerCase() === 'r' && primary && !mods.alt && !mods.shift) return 'rename'
  return null
}

/**
 * Where up/down/home/end move the selection, or `null` when the key is not one of ours.
 *
 * `null` rather than "stay where you are" so the caller can tell "handled, did not move" from
 * "not mine" — only the first may call `preventDefault`, and swallowing an unrelated key in a
 * tree is how Ctrl+F stops working inside a panel.
 *
 * Clamped rather than wrapped. A tree of 100 000 rows in which End is one keystroke from Home
 * is a tree in which overshooting the top costs you your place completely.
 */
export function moveIndex(key: string, at: number, count: number): number | null {
  if (count <= 0) return null
  const last = count - 1
  const from = clamp(at, 0, last)
  switch (key) {
    case 'ArrowDown':
      return Math.min(from + 1, last)
    case 'ArrowUp':
      return Math.max(from - 1, 0)
    case 'Home':
      return 0
    case 'End':
      return last
    default:
      return null
  }
}

function clamp(value: number, lo: number, hi: number): number {
  if (!Number.isFinite(value)) return lo
  return Math.min(Math.max(Math.trunc(value), lo), hi)
}
