/**
 * The mapping between what the diff view highlights and what `git_stage` is sent.
 *
 * This is the one module in the git panel where a bug destroys uncommitted work rather than
 * drawing a wrong number. `cide_git::patch::choose` turns a `Selection` into a set of
 * positions and `synthesize` builds a patch out of exactly those; if the set the user sees
 * highlighted and the set the `Selection` denotes ever differ, cide stages lines nobody
 * asked for and the user finds out at review time, or never.
 *
 * So the two directions are both written down here and checked against each other:
 *
 * * {@link highlightedKeys} is what the renderer paints. The pane paints *from this
 *   function* — it does not re-derive "is this row ticked" from the marks — so agreement is
 *   structural rather than a convention.
 * * {@link toSelection} is what travels to Rust.
 * * {@link resolve} is a faithful port of `patch::choose`, and exists only so the two can be
 *   compared. `ui/scripts/check-diff-selection.mjs` asserts
 *   `resolve(diff, toSelection(marks, diff)) === highlightedKeys(marks, diff)` over an
 *   exhaustive sweep of a small diff — every subset, not a sample.
 *
 * Pure, and deliberately React-free: the check script compiles this one file with `tsc` and
 * imports the output under node, exactly as `check-git-tree.mjs` does with `model.ts`. Its
 * imports must stay type-only for that to keep working.
 *
 * # Vocabulary
 *
 * A **mark** is a `"<hunk>:<line>"` key into `FileDiff.hunks[h].lines[l]`. Marks are stored
 * rather than `LineRef` objects because a `Set` of them de-duplicates and compares by value,
 * which is what makes a toggle one line. They are converted to `LineRef`s at the boundary.
 *
 * Context lines are never selectable. `patch::choose` drops them from a `Lines` selection
 * silently, so a UI that let one be ticked would show a highlighted row that stages nothing
 * — the smaller, quieter half of the same class of bug.
 */
import type { DiffHunkView, FileDiff, LineRef, PathSelection, Selection } from '@/ipc/generated'

/** `"<hunk>:<line>"`. */
export type Mark = string

export type Marks = ReadonlySet<Mark>

export const EMPTY_MARKS: Marks = new Set<Mark>()

export function mark(hunk: number, line: number): Mark {
  return `${hunk}:${line}`
}

/** Parse a mark back. Only ever called on marks this module produced. */
export function unmark(key: Mark): LineRef {
  const colon = key.indexOf(':')
  return { hunk: Number(key.slice(0, colon)), line: Number(key.slice(colon + 1)) }
}

/** Whether this row is an addition or a deletion — the only two that can be staged. */
export function isChange(hunk: DiffHunkView | undefined, line: number): boolean {
  const row = hunk?.lines[line]
  return row !== undefined && row.origin !== 'context'
}

/** Every selectable position in the file, in reading order. */
export function changeMarks(diff: FileDiff): Mark[] {
  const out: Mark[] = []
  diff.hunks.forEach((hunk, h) => {
    hunk.lines.forEach((line, l) => {
      if (line.origin !== 'context') out.push(mark(h, l))
    })
  })
  return out
}

/** Every selectable position in one hunk. */
export function hunkMarks(diff: FileDiff, hunk: number): Mark[] {
  const target = diff.hunks[hunk]
  if (target === undefined) return []
  const out: Mark[] = []
  target.lines.forEach((line, l) => {
    if (line.origin !== 'context') out.push(mark(hunk, l))
  })
  return out
}

/**
 * What the pane paints as selected.
 *
 * Marks are filtered against the diff rather than trusted: a refetch can shrink a hunk under
 * a stale mark, and a highlighted row that no longer exists is a row that would be sent as a
 * position into a shorter array. (The pane also clears its marks whenever `FileDiff.rev`
 * changes, so this is the second of two guards; the first one is the one that keeps the
 * *positions* meaningful, and this one only keeps them in range.)
 */
export function highlightedKeys(marks: Marks, diff: FileDiff): Set<Mark> {
  const out = new Set<Mark>()
  // A file `cide_git` refuses to stage in parts has no selectable positions at all — binary,
  // submodule, symlink, deletion, rename. The pane draws no boxes for one, so this cannot
  // normally hold anything; stating the rule here as well means a mark that survived from
  // before (a refetch that flipped `partialOk`) cannot light a row and enable a button whose
  // command would be refused. The refusal is Rust's; this is the same rule where the painting
  // happens.
  if (!diff.partialOk) return out
  for (const key of marks) {
    const { hunk, line } = unmark(key)
    if (isChange(diff.hunks[hunk], line)) out.add(key)
  }
  return out
}

/** The same set as `LineRef`s, in reading order — the wire form of `Selection::Lines`. */
export function highlightedRefs(marks: Marks, diff: FileDiff): LineRef[] {
  const live = highlightedKeys(marks, diff)
  return changeMarks(diff)
    .filter((key) => live.has(key))
    .map(unmark)
}

export type HunkState = 'none' | 'some' | 'all'

/** The hunk header's tri-state box. `none` for a hunk with nothing selectable in it. */
export function hunkState(marks: Marks, diff: FileDiff, hunk: number): HunkState {
  const all = hunkMarks(diff, hunk)
  if (all.length === 0) return 'none'
  const on = all.filter((key) => marks.has(key)).length
  if (on === 0) return 'none'
  return on === all.length ? 'all' : 'some'
}

/** Tick or untick one row. A context row is refused rather than stored. */
export function toggleLine(marks: Marks, diff: FileDiff, hunk: number, line: number): Set<Mark> {
  const next = new Set(marks)
  if (!isChange(diff.hunks[hunk], line)) return next
  const key = mark(hunk, line)
  if (!next.delete(key)) next.add(key)
  return next
}

/** Tick every row of a hunk, or clear it when it is already fully ticked. */
export function toggleHunk(marks: Marks, diff: FileDiff, hunk: number): Set<Mark> {
  const next = new Set(marks)
  const all = hunkMarks(diff, hunk)
  const full = hunkState(marks, diff, hunk) === 'all'
  for (const key of all) {
    if (full) next.delete(key)
    else next.add(key)
  }
  return next
}

export function selectEverything(diff: FileDiff): Set<Mark> {
  return new Set(changeMarks(diff))
}

/**
 * What to send for the highlighted rows, or `null` when nothing is highlighted.
 *
 * Three shapes, narrowest last, and the order is a safety ranking rather than a compression:
 *
 * 1. **`whole`** when every change in the file is selected. That is the index API —
 *    literally what `git add` does — so binaries, modes, symlinks, deletions and renames are
 *    exact by construction and no patch this codebase wrote is involved. `cide_git::stage`
 *    normalises to it anyway (`is_whole`); saying it here means the common "tick the file"
 *    gesture never reaches patch synthesis even in the argument.
 * 2. **`hunks`** when the selection is hunk-aligned, because that is the selection the user
 *    actually expressed and it survives being read in a log line.
 * 3. **`lines`** otherwise.
 *
 * All three denote the same set of positions — that is the invariant the check script pins.
 */
export function toSelection(marks: Marks, diff: FileDiff): Selection | null {
  const chosen = highlightedKeys(marks, diff)
  if (chosen.size === 0) return null

  const every = changeMarks(diff)
  if (every.length > 0 && chosen.size === every.length) return { kind: 'whole' }

  const hunks: number[] = []
  let covered = 0
  diff.hunks.forEach((_, h) => {
    const all = hunkMarks(diff, h)
    if (all.length > 0 && all.every((key) => chosen.has(key))) {
      hunks.push(h)
      covered += all.length
    }
  })
  if (hunks.length > 0 && covered === chosen.size) return { kind: 'hunks', hunks }

  return { kind: 'lines', lines: highlightedRefs(marks, diff) }
}

/**
 * The `PathSelection` for a diff, or `null` when nothing is selected.
 *
 * `rev` is where the staleness check lives, and it is attached to exactly the selections that
 * need it. A `Whole` selection names no positions, so it stays correct however the file moved
 * and carries `null` — the same thing the panel's whole-file checkboxes send. A `Hunks` or
 * `Lines` selection is nothing *but* positions, so it carries the rev of the diff it was made
 * against and `cide_git::stage::resolve` refuses it if the file moved underneath. Sending
 * `null` there would apply yesterday's line numbers to today's diff and stage different
 * lines, silently.
 */
export function pathSelection(marks: Marks, diff: FileDiff): PathSelection | null {
  const selection = toSelection(marks, diff)
  if (selection === null) return null
  return {
    path: diff.path,
    selection,
    rev: selection.kind === 'whole' ? null : diff.rev,
  }
}

/**
 * Which positions a `Selection` denotes — a port of `cide_git::patch::choose`.
 *
 * Kept faithful to that function rather than convenient, including that `Lines` silently
 * drops context rows and that `Hunks` takes every change in the named hunk. A position the
 * diff no longer has is dropped here; Rust raises `StaleSelection` for it, which is the
 * stricter of the two and the one that acts.
 */
export function resolve(diff: FileDiff, selection: Selection | null): Set<Mark> {
  if (selection === null) return new Set()
  switch (selection.kind) {
    case 'whole':
      return new Set(changeMarks(diff))
    case 'hunks': {
      const out = new Set<Mark>()
      for (const h of selection.hunks) for (const key of hunkMarks(diff, h)) out.add(key)
      return out
    }
    case 'lines': {
      const out = new Set<Mark>()
      for (const ref of selection.lines) {
        if (isChange(diff.hunks[ref.hunk], ref.line)) out.add(mark(ref.hunk, ref.line))
      }
      return out
    }
  }
}

/** How many lines a stored selection covers — what the panel's note counts. */
export function countLines(diff: FileDiff, selection: Selection | null): number {
  return resolve(diff, selection).size
}
