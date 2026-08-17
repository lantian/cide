/**
 * Back and Forward: an ordered list of the places the user jumped to, and a cursor into it.
 *
 * The arithmetic only. `jump.ts` is the shell that holds one of these per project, reads the
 * live caret and drives `requestReveal` — it contains no decisions, which is why it is not
 * compiled by a check and this file is.
 *
 * # The recording rule, in the words of what it deliberately does not record
 *
 * **Only an explicit navigation writes an entry.** A jump is something the user asked for by
 * name: Go to definition, a search-result click, a Problems-panel click, the file picker, the
 * File Structure popup, Go to symbol, opening a file from the Explorer, Ctrl+clicking a path in
 * terminal output. Every one of those goes through `jump.ts::jumpTo` and nothing else does.
 *
 * These are **not** recorded, and each is a decision rather than an omission:
 *
 * * **Typing, arrow keys, Home/End, PageUp/PageDown.** No caret motion is ever an entry. A
 *   history that records caret moves and filters them by distance is a history where Back means
 *   "undo the last few keystrokes of movement", which is what makes Back useless — and it is
 *   also untestable, because the filter lives in an update listener that fires thirty times a
 *   second under a held key.
 * * **Scrolling, the wheel, minimap drags.** An entry is a caret, and a scroll does not move
 *   one. (The *view memory* does follow scrolling. That is the other feature, with the other
 *   store; see `position.ts`.)
 * * **A far pointer click.** IDEA records one and this does not — the honest reason is that
 *   the rule would have to live inside `EditorSurface`'s update listener, and that component is
 *   documented as pure: text in, text out, no IPC, no store, no knowledge of tabs. Teaching it
 *   what a navigation history is to gain a heuristic (*"more than N lines away counts"*) is a
 *   worse trade than the missing entry. It is the one item on this list that might be worth
 *   revisiting, and it is written down rather than left to be rediscovered.
 * * **Find, find-as-you-type and Find Next.** One entry per keystroke of a query is precisely
 *   the flood the rule exists to prevent.
 * * **`navigate.nextMember` / `navigate.prevMember`.** Bound to `alt+up`/`alt+down`, which is a
 *   held key: ten presses would be ten entries, and the merge rule below cannot collapse them
 *   because members are far apart by construction. Whatever brought the user into the file
 *   recorded an entry, so the origin of the walk is still reachable.
 * * **Switching between already-open tabs** (Ctrl+Tab, clicking the strip). Opening a file that
 *   was not open is a jump; re-activating a tab the user can already see is not.
 *
 * Opening a file with **no** position — the Explorer, the file picker, open-in-split — *is*
 * recorded, and it needs [`UNKNOWN_LINE`] to be honest about it: at the moment of the click
 * nobody knows where that file will open, because the per-file view memory decides that after
 * the editor mounts. Recording line 1 would be a lie that Forward would later act on.
 * * **Edits.** IDEA's *Last Edit Location* is a separate command with a separate stack.
 * * **A Back or Forward move itself.** Walking the history must not push onto it, or Back
 *   becomes a machine that can only ever go back one step. That falls out of [`walk`] being a
 *   different function from [`record`], which is why they are two functions.
 *
 * # Why this is not merged with the view memory
 *
 * They share `FilePosition` and nothing else, and the lifetimes are opposites:
 *
 * | | view memory | this |
 * | --- | --- | --- |
 * | shape | a map, one entry per path, overwritten | an ordered list plus a cursor, many per path |
 * | lifetime | persisted across restarts | session-scoped, per window |
 * | trim | LRU at 256 | by count, at [`NAV_CAP`], oldest first |
 * | written on | every scroll and caret settle | an explicit jump, and nothing else |
 *
 * A map with an order field is a stack pretending to be a map; a stack keyed by path destroys
 * the ordering that back/forward entirely consists of. Merging them would force one to adopt
 * the other's shape, and either direction produces something useless.
 */
import { samePosition, type FilePosition } from './position'

export type NavDirection = 'back' | 'forward'

/**
 * The visited places, oldest first, and where the user is standing in them.
 *
 * `index` addresses the entry the user is *at*, so `index - 1` is Back and `index + 1` is
 * Forward — browser semantics. `-1` means the history is empty, which is also the only state in
 * which `entries` is empty.
 *
 * Immutable: every operation returns a new value. The store in `jump.ts` is a plain module-level
 * `Map`, and a value that could be mutated in place would make "did anything change" a question
 * with no answer.
 */
export interface NavHistory {
  readonly entries: readonly FilePosition[]
  readonly index: number
}

export const EMPTY_HISTORY: NavHistory = { entries: [], index: -1 }

/**
 * Two places this close together, in the same file, are one entry.
 *
 * Without it Back walks five entries to get out of one function: Go to definition inside the
 * file you are already reading, then the structure popup to the method below it, then a search
 * hit two lines further on, and the way back out is four presses through places that all look
 * identical on screen. Three lines is about a signature and its first statement.
 */
export const MERGE_LINES = 3

/**
 * How many entries are kept.
 *
 * Fifty, dropping from the *front*. This is a session's worth of jumping and it is not drawn
 * anywhere, so the cap is about memory rather than legibility — but it is much smaller than the
 * view memory's 256 because an unbounded back-stack is a slow leak in a process that stays up
 * for days, and because nobody has ever pressed Back fifty times.
 */
export const NAV_CAP = 50

/**
 * The `line` of an entry whose position is not known: *this file, wherever it opens*.
 *
 * Zero rather than a nullable field, because every consumer of a `FilePosition` already has to
 * cope with a line that is out of range — lines are 1-based, so 0 is out of range by
 * construction and every clamp in the app already handles it. A second optional field would
 * have to be threaded through `jumpTo`, the store and the walk, and forgotten in one of them.
 *
 * It appears for exactly one gesture: opening a file with no position (the Explorer, the file
 * picker, open-in-split). At the moment of that click nobody can know where the file will open,
 * because the per-file view memory decides that after the editor mounts. So the entry says "I do
 * not know" and two things make it right:
 *
 * * [`near`] treats it as matching *any* position in the same file, so the moment the user jumps
 *   away — or walks past it — the live caret replaces it with a real one, in place.
 * * `jump.ts` reveals nothing for such an entry: it opens the file and lets the view memory
 *   decide, which is the same thing the original click did.
 */
export const UNKNOWN_LINE = 0

/** Same file, close enough that a second entry would be noise. */
function near(a: FilePosition, b: FilePosition): boolean {
  if (a.path !== b.path) return false
  // "Somewhere in this file" is the same place as any place in this file — see `UNKNOWN_LINE`.
  if (a.line === UNKNOWN_LINE || b.line === UNKNOWN_LINE) return true
  return Math.abs(a.line - b.line) <= MERGE_LINES
}

/**
 * Record a jump from `origin` (where the user was, or `null` if that is unknown) to
 * `destination`.
 *
 * # What happens to the forward tail
 *
 * It is discarded, exactly as a browser discards it: having gone Back twice and then jumped
 * somewhere new, the two places that were ahead of you are no longer anywhere you can get to by
 * pressing Forward. Keeping them would mean Forward sometimes goes somewhere you have never
 * been from here.
 *
 * # Why the origin is written now rather than when the user arrived
 *
 * `origin` is the *live* caret, read at the moment of the jump. The entry already at the cursor
 * says where the user was when they arrived in that file, which may be several screens away
 * from where they are when they leave it. Refreshing it — rather than pushing a second entry —
 * is what makes Back-then-Forward return you to where you actually were.
 */
export function record(
  history: NavHistory,
  origin: FilePosition | null,
  destination: FilePosition,
): NavHistory {
  // The forward tail goes first, so everything below appends to a list whose last element is
  // the entry the user is standing on.
  const entries = history.entries.slice(0, history.index + 1)

  if (origin !== null) push(entries, origin)
  push(entries, destination)

  return cap({ entries, index: entries.length - 1 })
}

/** Replace the last entry when it is the same place, append otherwise. */
function push(entries: FilePosition[], at: FilePosition): void {
  const last = entries[entries.length - 1]
  if (last !== undefined && near(last, at)) entries[entries.length - 1] = at
  else entries.push(at)
}

/** Hold the list at [`NAV_CAP`], dropping the oldest and moving the cursor with them. */
function cap(history: NavHistory): NavHistory {
  const over = history.entries.length - NAV_CAP
  if (over <= 0) return history
  return {
    entries: history.entries.slice(over),
    // The cursor addresses an entry, so dropping `over` entries off the front moves it by
    // `over` — not clamping it to 0, which would silently teleport the user to the start of
    // their own history the first time the cap was reached.
    index: Math.max(0, history.index - over),
  }
}

/** Whether there is anywhere to go in `direction`. What a disabled Back button would read. */
export function canWalk(history: NavHistory, direction: NavDirection): boolean {
  const target = direction === 'back' ? history.index - 1 : history.index + 1
  return target >= 0 && target < history.entries.length
}

/**
 * Move the cursor one step, and say where that lands.
 *
 * `null` when there is nowhere to go — the caller reports that rather than doing nothing, per
 * this project's rule that a gesture with no visible effect is indistinguishable from a control
 * wired to nothing.
 *
 * `live` is the caret as it is *now*, and it refreshes the entry being left — but only when it
 * is in the same file. A caret in a different file means the user changed tabs by hand since
 * arriving, and overwriting the entry with an unrelated place would corrupt the stack in a way
 * that only shows up several presses later.
 *
 * **This does not push.** A Back that recorded itself could never reach the entry before last.
 */
export function walk(
  history: NavHistory,
  direction: NavDirection,
  live: FilePosition | null,
): { history: NavHistory; to: FilePosition } | null {
  const target = direction === 'back' ? history.index - 1 : history.index + 1
  const to = history.entries[target]
  if (to === undefined) return null

  const entries = history.entries.slice()
  const current = entries[history.index]
  if (live !== null && current !== undefined && current.path === live.path) {
    entries[history.index] = live
  }
  return { history: { entries, index: target }, to }
}

/**
 * Undo the walk just made in `direction`, and drop the entry it landed on: that file is gone.
 *
 * Called when a walk arrives somewhere the disk no longer has — `tab_reopen_file` answers `gone`.
 * Without it the entry stays and the *next* press offers the same dead file for ever, which is
 * the dead key `tab_reopen_closed` already refuses to be (`reopen_plan` drops a deleted record
 * and tries the next one). Back should not be the one gesture in the app that keeps walking into
 * a wall.
 *
 * # Why this takes a direction rather than an index
 *
 * Because the cursor has to go back to **where the user actually is**, and after a refused walk
 * that is *not* recoverable from the history alone. [`walk`] has already moved the cursor onto
 * the entry that turned out to be dead; the user, meanwhile, never went anywhere — they are
 * still looking at the entry they walked *from*, which is `index + 1` for a Back and `index - 1`
 * for a Forward. Only the direction says which.
 *
 * The first version of this was `forget(history, index)`: remove the entry, and let the cursor
 * sit where the removal leaves it. That is right for Back by coincidence and **wrong for
 * Forward**, which is the sort of half-correct that survives a manual test. With `[A, B, C]` and
 * the user standing on `A`, Forward into a deleted `B` left the cursor on `C` — a place they had
 * not been. Forward then refused ("already at the most recent place") while `C` was live and one
 * press away, and Back returned to `A`, which they were already looking at, so it read as a
 * button that does nothing. Both symptoms of one wrong index.
 *
 * The removal shifts everything after the dead entry down by one, so the home position moves with
 * it when it was to the right and stays put when it was to the left. Clamped, so it can never
 * point past the end; an emptied history returns to [`EMPTY_HISTORY`] rather than to
 * `{ entries: [], index: 0 }`, which [`isCoherent`] would reject.
 */
export function abandon(history: NavHistory, direction: NavDirection): NavHistory {
  const dead = history.index
  if (dead < 0 || dead >= history.entries.length) return history
  const entries = history.entries.slice(0, dead).concat(history.entries.slice(dead + 1))
  if (entries.length === 0) return EMPTY_HISTORY

  // Where the user is standing: the entry the walk came from, before the removal renumbers it.
  const home = direction === 'back' ? dead + 1 : dead - 1
  const shifted = home > dead ? home - 1 : home
  return { entries, index: Math.min(Math.max(shifted, 0), entries.length - 1) }
}

/**
 * Whether `history` is in a state it could not have reached by construction. For checks only.
 *
 * A fuzz walk over thousands of random operations asserts this stays true, because the one
 * failure mode of an index-into-an-array is an index that is out of it — and the symptom would
 * be Back doing nothing for ever, with no error anywhere.
 */
export function isCoherent(history: NavHistory): boolean {
  if (history.entries.length === 0) return history.index === -1
  return (
    history.index >= 0 &&
    history.index < history.entries.length &&
    history.entries.length <= NAV_CAP
  )
}

/** Re-exported so a caller does not need two imports to compare two places. */
export { samePosition }
