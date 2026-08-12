/**
 * Dragging rows between changelists: which rows a gesture carries, which targets accept it,
 * and what a drop actually does.
 *
 * > *"git changes tree - i should be able to drag one or multiple selected elements
 * > (including whole directories) to another changelist"*
 *
 * Every rule is a plain function over plain data — no React, no DOM, no IPC — for the same
 * reason `model.ts` is: `ui/scripts/check-git-tree.mjs` runs it under node, and the rules here
 * are the half of drag and drop that is a decision rather than a mechanism. The mechanism (the
 * pointer state machine) is `useChangesDrag.ts`, and nothing in this repo can execute it.
 *
 * # The three decisions, up front
 *
 * **What a grab carries.** A file row that is *ticked* carries every tick in the same repo and
 * the same list kind — that is `actOn`, which the *Move to Changelist…* menu item has used
 * since M8, and a drag that disagreed with the menu on the same row would be the surprise.
 * A file row that is not ticked carries itself alone and **changes no ticks**: in this panel a
 * tick is what a commit will take, not what the pointer is on (see the header of
 * `ChangesTree.tsx`), so a drag that ticked what it touched would silently change what the
 * Commit button does. A directory row carries the files under it and never widens — the row
 * already names a set, and widening from it would move files that are not under the row the
 * user grabbed.
 *
 * The alternative that lost: IDEA's ctrl-click row multi-selection, separate from the
 * checkboxes, with the drag following *that*. It is the better model and it is what IDEA does
 * — but this panel has no such selection, and inventing one changes what every click in the
 * tree means. Ticks it is, with the count on the drag ghost from the first pixel of movement so
 * a widened drag is never invisible. Note the trap it comes with: the active changelist opens
 * fully ticked, so dragging one of its files carries all of them. The ghost says so, ⎋ cancels,
 * and a changelist move loses nothing — it is the one write in this panel that is undone by
 * dragging back.
 *
 * **Which targets accept.** Only a changelist. Any row inside one resolves to the group it is
 * in, so the whole band of a changelist is a target rather than its 23px header; a repository
 * row, another repository's list, and the three sibling lists (`Unversioned Files`,
 * `Ignored Files`, `Merge Conflicts`) refuse, each with the reason written here and drawn on
 * the ghost. Refusing visibly matters more than usual for the sibling lists, because filing an
 * untracked path *succeeds* in the sidecar and is then dropped by the next status walk — a
 * silent no-op that looks exactly like a broken feature.
 *
 * **What a same-list drop does.** Nothing, and says so. `paths` holds only the files that
 * actually change list, so a drop that moves three of five files sends three, and a drop where
 * every file is already there sends nothing at all — no command, no busy line, no flash.
 */
import { changelistIdOf, flatFiles, type Row } from './model'
import type { GroupKind, RepoId, StatusView } from './types'

/** One file under the pointer's care. */
export interface DraggedFile {
  /** Its row id — what the tree dims while the drag is in flight. */
  id: string
  /** Repo-relative, which is what `git_changelist_move_paths` takes. */
  path: string
  /** The changelist it is in *now*, so a same-list drop can be recognised. */
  changelist: string
}

/** What a grab picked up. */
export interface DragSet {
  repo: RepoId
  /** Which of the four lists these came from. Only `changelist` can be moved. */
  kind: GroupKind
  /** The row the gesture started on. */
  origin: string
  files: DraggedFile[]
  /** What the ghost calls the load: a basename, a directory, or `4 files`. */
  label: string
  /** True when the grab widened past the row it started on — the fact users must see. */
  widened: boolean
}

/**
 * What a gesture on `row` picks up, or `null` when that row is not a drag source.
 *
 * Repo rows and group rows are `null`, deliberately, and for two different reasons. A
 * repository is not in any changelist and its rows span its submodules, whose files belong to
 * a different sidecar. A changelist header is not a source because a press on one *folds it*
 * — that is the click rule this tree already had (`gitTreeClick`), and a drag that began by
 * collapsing the row under the pointer would be unusable; moving a whole changelist is the
 * group menu's `Move N Files to Changelist…`, one keystroke away and with a count to read.
 */
export function grab(
  view: StatusView,
  selected: ReadonlySet<string>,
  row: Row,
): DragSet | null {
  if (row.kind !== 'file' && row.kind !== 'dir') return null
  const kind = row.groupKind
  if (kind === undefined) return null

  const all = flatFiles(view)
  const byId = new Map(all.map((f) => [f.id, f]))
  const own = row.files.flatMap((id) => {
    const found = byId.get(id)
    return found === undefined ? [] : [{ id, path: found.entry.path, changelist: found.changelist }]
  })
  if (own.length === 0) return null

  // Widening is for file rows only. A directory row already names its set; sweeping the ticks
  // in from outside it would move files that are not under the row the user grabbed.
  const widen = row.kind === 'file' && selected.has(row.id)
  const files = widen
    ? all.flatMap((f) =>
        f.repo === row.repo && f.kind === kind && selected.has(f.id)
          ? [{ id: f.id, path: f.entry.path, changelist: f.changelist }]
          : [],
      )
    : own
  // `actOn` has the same guard: the widening must never come back empty, or a right-click on a
  // ticked row would act on nothing at all.
  const carried = files.length > 0 ? files : own
  return {
    repo: row.repo,
    kind,
    origin: row.id,
    files: carried,
    label: labelFor(row, carried.length, carried.length > own.length),
    widened: carried.length > own.length,
  }
}

/** `3 files`, `GitPanel.tsx`, `ui/src/sidebar` — what the ghost carries under the pointer. */
function labelFor(row: Row, count: number, widened: boolean): string {
  if (widened || (row.kind === 'file' && count > 1)) return plural(count)
  if (row.kind === 'dir') return `${row.path ?? row.label}/`
  return row.label
}

export function plural(n: number): string {
  return `${n} ${n === 1 ? 'file' : 'files'}`
}

/**
 * The changelist row a drop at `id` would land on, or `null`.
 *
 * Any row inside a group resolves to that group, so the drop target is the whole band of a
 * changelist rather than the one 23px header row — a 420px panel where the header has scrolled
 * off the top is exactly where a drop is needed most. A repository row resolves to nothing: it
 * sits *above* its groups, so walking back from it would find the previous repository's last
 * changelist and file the paths into the wrong repo's sidecar.
 */
export function dropTarget(rows: readonly Row[], id: string | null): Row | null {
  if (id === null) return null
  const at = rows.findIndex((row) => row.id === id)
  const row = rows[at]
  if (row === undefined || row.kind === 'repo') return null
  for (let i = at; i >= 0; i--) {
    const candidate = rows[i]
    if (candidate === undefined || candidate.kind !== 'group') continue
    // Only the group of the row we started from. In a multi-root workspace the scan would
    // otherwise cross a repository boundary and offer a list that cannot hold these paths.
    return candidate.repo === row.repo ? candidate : null
  }
  return null
}

/** What dropping `drag` on a target would do. `hint` is the sentence the ghost draws. */
export type DropOutcome =
  | {
      kind: 'move'
      repo: RepoId
      /** The raw changelist id `git_changelist_move_paths` takes — never the `cl:` group id. */
      changelist: string
      /** The list's name, for the sentence. */
      list: string
      /** Only the paths that actually change list. */
      paths: string[]
      hint: string
    }
  | { kind: 'noop'; list: string; hint: string }
  | { kind: 'refuse'; reason: string; hint: string }

function refuse(reason: string): DropOutcome {
  return { kind: 'refuse', reason, hint: reason }
}

/**
 * The verdict for one (grab, target) pair.
 *
 * Source refusals are reported before target refusals: a set that cannot be filed anywhere
 * gives the same answer over every row, and saying so once teaches faster than a different
 * complaint per changelist.
 */
export function dropOutcome(drag: DragSet, target: Row | null): DropOutcome {
  if (drag.kind !== 'changelist') {
    // The wording the context menu already uses for the same refusal, so the two routes to
    // this operation do not explain themselves differently.
    return refuse(
      drag.kind === 'conflicts'
        ? 'Resolve the conflict first — conflicts are listed apart from changelists'
        : 'Only tracked changes belong to a changelist',
    )
  }
  if (target === null) return refuse('Drop on a changelist')
  if (target.repo !== drag.repo) {
    return refuse('A changelist belongs to one repository — these files are in another')
  }

  const id = changelistIdOf(target.group)
  if (id === null) {
    return refuse(`“${target.label}” is not a changelist — nothing can be filed there`)
  }

  const paths = drag.files.flatMap((f) => (f.changelist === id ? [] : [f.path]))
  if (paths.length === 0) {
    return {
      kind: 'noop',
      list: target.label,
      hint: `Already in “${target.label}”`,
    }
  }
  return {
    kind: 'move',
    repo: drag.repo,
    changelist: id,
    list: target.label,
    paths,
    hint: `Move ${plural(paths.length)} to “${target.label}”`,
  }
}

/**
 * Does this row belong to the load being dragged?
 *
 * Used to dim the source rows, and it has to answer for group and directory rows too — a
 * directory whose every file is in flight should not look untouched. `some` rather than
 * `every`: a half-carried directory is still involved, and drawing it as uninvolved would
 * suggest the files under it are staying put.
 */
export function inDrag(ids: ReadonlySet<string>, row: Row): boolean {
  return row.files.some((id) => ids.has(id))
}

/**
 * Which group row each row belongs to — row id to group row id.
 *
 * The drop target is a changelist, and in a 420px panel its header is very often scrolled off
 * the top by the time the pointer is over its files. Highlighting only the header would then
 * highlight nothing at all, which is the state in which people let go over the wrong list. One
 * pass in row order, because that is exactly the walk `dropTarget` does per row and this is
 * asked for every row of every frame of a drag.
 */
export function bands(rows: readonly Row[]): Map<string, string> {
  const out = new Map<string, string>()
  let group: Row | null = null
  for (const row of rows) {
    if (row.kind === 'group') group = row
    else if (row.kind === 'repo') group = null
    if (group !== null && group.repo === row.repo) out.set(row.id, group.id)
  }
  return out
}

/** The ids `inDrag` tests against, built once per drag rather than once per row. */
export function draggedIds(drag: DragSet): Set<string> {
  return new Set(drag.files.map((f) => f.id))
}
