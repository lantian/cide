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
 * **What a grab carries.** A row that is part of the **row selection** carries the whole
 * selection, narrowed to the same repository and the same list kind. A row that is not carries
 * itself alone — a file, or every file under a directory — and **changes nothing**: not the
 * ticks, and not the selection either. The press that preceded the drag has already put the
 * selection where it belongs (`rowSelection.ts::pressSelect` selects an unselected row on
 * mousedown, which is IDEA's rule and the reason this function never has to write anything).
 *
 * This used to widen by **ticks**, and that was the wrong set — the header here argued for it
 * at length and the argument was wrong in a way worth keeping on the record. It ran: this
 * panel has no row selection, inventing one changes what every click means, so use the one
 * multi-row concept that exists. The trap it came with was written down and shipped anyway:
 * the active changelist opened *fully ticked* (`model.ts::defaultSelection`, removed in M31
 * when ticks became opt-in), so dragging one file out of it moved every file in it. The
 * default is gone and the argument is not: ticks are still the wrong set to widen a drag by,
 * because a user who has ticked five files for a commit is not pointing at five files. That is
 * not a rough edge, it is the feature doing
 * something the user did not ask for, and it was reported as one. A tick is a statement about
 * a commit; a drag is a statement about what you are pointing at. `rowSelection.ts` is the
 * third concept, the click meanings are all in it, and `check:git` executes them.
 *
 * The count stays on the drag ghost from the first pixel of movement, so a widened drag is
 * still never invisible.
 *
 * **Which targets accept.** Only a changelist. Any row inside one resolves to the group it is
 * in, so the whole band of a changelist is a target rather than its 24px header; a repository
 * row, another repository's list, and the three sibling lists (`Unversioned Files`,
 * `Ignored Files`, `Merge Conflicts`) refuse, each with the reason written here and drawn on
 * the ghost. Refusing visibly matters more than usual for the sibling lists, because filing a
 * path git does not track *succeeds* in the sidecar and is then dropped by the next status
 * walk — a silent no-op that looks exactly like a broken feature.
 *
 * **What a same-list drop does.** Nothing, and says so. `paths` holds only the files that
 * actually change list, so a drop that moves three of five files sends three, and a drop where
 * every file is already there sends nothing at all — no command, no busy line, no flash.
 *
 * # Unversioned files are a second verb, not a refused move
 *
 * > *"i should be able to move files from Unversioned Files to any of change list via drag and
 * > drop and via context, currently it doesn't allow and in context it writes that files are
 * > not staged - but when dragging it should stage them automatically."*
 *
 * A drag out of `Unversioned Files` used to hit the same refusal as `Ignored Files` — *"Only
 * tracked changes belong to a changelist"* — which is a true sentence about the state of the
 * world and a useless one about the gesture, because the thing the user is asking for is
 * precisely to change that state. IDEA answers it by *adding the file to git*, and so does
 * this now: the drop is `track`, a two-step operation (add the paths to the index, then file
 * them) shown as one gesture.
 *
 * It is a **separate outcome kind** rather than a `move` with a flag, for one reason: adding a
 * path to git writes to the repository, and every branch that draws a verdict — the ghost, the
 * target ring, the context-menu label — has to be able to say so. A boolean on `move` would
 * have made "add these three files to git" a detail that any of those three could forget to
 * mention, and a drag that silently `git add`s is the failure this variant exists to prevent.
 * See `useGitPanel::trackPaths` for what it costs in the repository, and ADR 0004 for why the
 * index entry it creates does not survive committing a *different* changelist.
 *
 * `ignored` keeps the old refusal, verbatim. An ignored path dropped on `Changes` would be a
 * very surprising way to un-ignore something, and the sentence is still exactly true of it.
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
  /**
   * Which of the four lists these came from — **one** kind for the whole load.
   *
   * That is not a simplification, it is enforced by `grab`: the widening filters on
   * `f.kind === kind`, so a selection spanning `Changes` and `Unversioned Files` is narrowed
   * to whichever list the grabbed row sits in. It is what makes a mixed drop impossible to
   * represent, and therefore what lets `dropOutcome` answer with one verb: `changelist` is a
   * `move`, `unversioned` is a `track`, and the other two refuse.
   *
   * The alternative — carry both kinds and add the untracked half to git while merely
   * re-filing the tracked half — was rejected because the ghost would then have to describe
   * two operations at once over a set the user cannot see the composition of. Dragging from
   * the tracked row and again from the untracked one is two gestures, each of which says
   * plainly what it does.
   */
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
  /**
   * The row selection, resolved by `rowSelection.ts::carriedIds` — every selected row id plus
   * every file under a selected directory. Not the ticks; see the header.
   */
  carried: ReadonlySet<string>,
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

  /*
   * Directory rows widen now, where under the ticks they deliberately did not.
   *
   * The old rule was "a directory already names its set, and sweeping the ticks in from
   * outside it would move files that are not under the row the user grabbed" — true of ticks,
   * which are set by a checkbox somewhere else and may name half the tree. It is not true of a
   * selection: the other rows in it are rows the user just clicked, they are drawn with the
   * selection band, and refusing to carry them would make a folder the one row kind that
   * silently drops the rest of a multi-row drag.
   */
  const widen = carried.has(row.id)
  const files = widen
    ? all.flatMap((f) =>
        f.repo === row.repo && f.kind === kind && carried.has(f.id)
          ? [{ id: f.id, path: f.entry.path, changelist: f.changelist }]
          : [],
      )
    : own
  // The widening must never come back empty, or a right-click on a selected row whose
  // neighbours are all in another repository would act on nothing at all.
  const load = files.length > 0 ? files : own
  return {
    repo: row.repo,
    kind,
    origin: row.id,
    files: load,
    label: labelFor(row, load.length, load.length > own.length),
    widened: load.length > own.length,
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
 * changelist rather than the one 24px header row — a 420px panel where the header has scrolled
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
  | {
      /**
       * Add these paths to git, *then* file them — the `Unversioned Files` drop.
       *
       * Deliberately not a `move` carrying a flag; see the header. Everything that draws a
       * verdict switches on `kind`, so a new kind is what forces each of them to be told
       * about the write to the repository rather than inheriting the move's wording.
       */
      kind: 'track'
      repo: RepoId
      changelist: string
      list: string
      /**
       * Every dragged path, and all of them are staged.
       *
       * Not filtered against `DraggedFile.changelist` the way `move` is, and that is a fix
       * rather than an omission. `status::repo_changes` sets `changelist` from
       * `Sidecar::owner_of`, which answers *the active list* for any path nobody has filed —
       * untracked paths included, even though they are drawn under `Unversioned Files` and
       * are in no changelist at all. Filtering on it would make a drop onto the active
       * changelist — by far the most likely target — compute an empty set and report
       * `Already in “Changes”` over a file that is not in `Changes` and is not in git.
       */
      paths: string[]
      hint: string
    }
  | { kind: 'noop'; list: string; hint: string }
  | { kind: 'refuse'; reason: string; hint: string }

function refuse(reason: string): DropOutcome {
  return { kind: 'refuse', reason, hint: reason }
}

/**
 * The clause that admits the write to the repository, for the ghost.
 *
 * Written once and read by both routes to this operation. The context menu cannot say the
 * whole sentence — it opens the chooser, so it does not know the list yet — but it must make
 * the same claim, which is why `trackMenuLabel` below is its sibling rather than its own
 * string somewhere in `GitPanelHost`. The two sentences drifting apart is how one route ends
 * up promising a re-filing and performing a `git add`.
 */
export function trackHint(count: number, list: string): string {
  return `Add ${plural(count)} to git and move to “${list}”`
}

/** The same claim, as a context-menu item. Title case, and a `…` because a chooser follows. */
export function trackMenuLabel(count: number): string {
  return `Add ${plural(count)} to Git and Move to Changelist…`
}

/**
 * The verdict for one (grab, target) pair.
 *
 * Source refusals are reported before target refusals: a set that cannot be filed anywhere
 * gives the same answer over every row, and saying so once teaches faster than a different
 * complaint per changelist.
 */
export function dropOutcome(drag: DragSet, target: Row | null): DropOutcome {
  if (drag.kind !== 'changelist' && drag.kind !== 'unversioned') {
    // The wording the context menu already uses for the same refusal, so the two routes to
    // this operation do not explain themselves differently. `unversioned` used to land here
    // too and no longer does — it has a verb now, one row down.
    //
    // Written as "not one of the two that have a verb" rather than as "conflicts or ignored",
    // which reads better and fails the wrong way: a fifth `GroupKind` added later would fall
    // past both branches and be treated as a plain `move`, silently writing an assignment for
    // a set of paths nobody has decided belongs in a changelist. Refusing an unknown kind
    // costs a wrong-ish sentence on a list that does not exist yet; admitting it costs a write.
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

  if (drag.kind === 'unversioned') {
    const paths = drag.files.map((f) => f.path)
    return {
      kind: 'track',
      repo: drag.repo,
      changelist: id,
      list: target.label,
      paths,
      hint: trackHint(paths.length, target.label),
    }
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
