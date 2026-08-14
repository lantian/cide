/**
 * Dragging file-tree rows into a folder: what a grab picks up, which rows accept it, what a
 * drop means, and every refusal it has to say out loud.
 *
 * > *"drag selected elements to some folder"*
 *
 * The sibling of `GitPanel/dragDrop.ts`, and deliberately the same shape: every *decision* is a
 * plain function over plain data — no React, no DOM, no IPC — so `ui/scripts/check-tree-drag.mjs`
 * can run it under node. The *mechanism* (the pointer state machine, the 4px threshold, the
 * spring-loaded folders) is `useTreeDrag.ts`, and nothing in this repo can execute that. This
 * project has paid four times for a rule that lived in an event handler, and a drag that moves
 * the user's files with no undo anywhere is the last place to pay a fifth.
 *
 * # Nothing here is new policy
 *
 * That is the design, not a coincidence. A drop **is** a Cut followed by a Paste, so every rule
 * below is the one the menu already applies, called rather than restated:
 *
 * * what may be picked up — `rowPaths.mutationRefusal` plus `isRootPath`, exactly what greys
 *   *Cut* (`FileTree.tsx::cutAction`);
 * * where a drop lands — `newEntry.targetFor`, exactly what *New File…* and *Paste* use, so a
 *   file row means its **parent** in all three and a folder row means the folder;
 * * whether that destination may be filled — `rowPaths.creationRefusal`, exactly what refuses
 *   *New File in…* and *Paste* inside the *Scratches* drawer;
 * * "into itself" — `clipboardModel.isInside`, which the module header there already names as
 *   the single deliberate overlap with `cide_fs::copy::check_source`.
 *
 * A second copy of any of those would be a second answer to a question the user asks two ways,
 * and the one that would go wrong is this one: it has no dialog by default and no undo at all.
 *
 * # Why a refusal is a sentence and not a `false`
 *
 * Every `refuse` below carries the words drawn on the ghost under the pointer. A drag that
 * simply does not land is indistinguishable from a drag the app ignored, and the two refusals
 * that matter most — a folder into its own descendant, and a drop onto *External Libraries* —
 * are both gestures a user makes on purpose and needs an answer to. The colour says "no" to
 * everyone who can see it; the sentence is the half that says *why*.
 */
import { isInside } from './clipboardModel'
import { basenameOf, dirnameOf, targetFor } from './newEntry'
import { creationRefusal, isRootPath, mutationRefusal, rootOf } from './rowPaths'
import { isSyntheticPath, rowVerbs, type RowKind } from './groupRows'

/**
 * The parts of a `TreeRow` a drag reads.
 *
 * Structural, so the generated `TreeRow` satisfies it and this module imports nothing from the
 * wire types — the same arrangement `clipboardModel.PasteOutcome` uses. `name` is here for one
 * reason: a group header's `path` is a `cide://group/…` sentinel that names nothing, so the
 * refusal it earns has to be phrased from the label the user actually clicked.
 */
export interface DragRow {
  readonly path: string
  readonly kind: RowKind
  readonly name: string
  readonly expanded: boolean
  readonly hasChildren: boolean
}

/** What a grab picked up. */
export interface TreeDragSet {
  /** Absolute paths, in selection order. What `fs_paste` is given as `sources`. */
  readonly paths: readonly string[]
  /** The row the gesture started on. */
  readonly origin: string
  /** What the ghost calls the load: a basename, `src/`, or `4 items`. */
  readonly label: string
  /** True when the grab widened past the row it started on — the fact the user must see. */
  readonly widened: boolean
  /**
   * Why nothing in this load can move, or `null`.
   *
   * Carried rather than turned into a `null` grab, and the difference is the whole reason this
   * field exists. A project root and a dependency source are *files* — the user grabbed
   * something real — so the honest answer is a drag that starts and says why it cannot land,
   * over every target, once. A grab that silently refuses to begin is indistinguishable from a
   * broken feature, which is the failure mode this repository keeps shipping.
   */
  readonly refusal: string | null
}

/** `1 item` / `4 items` — the vocabulary *Cut 3 Items* and *Move 3 items to Trash* already use. */
export function plural(n: number): string {
  return n === 1 ? '1 item' : `${n} items`
}

/**
 * What a press on `row` picks up, or `null` when that row is not a drag source at all.
 *
 * A row that is part of the **selection** carries the whole selection; a row that is not carries
 * itself alone and changes nothing. The press that preceded the drag has already put the
 * selection where it belongs — `treeSelection.pressSelect` selects an unselected row on
 * mousedown and *defers* the collapse of a multi-row selection to the release, which is the one
 * change this feature needed in that module and the reason a multi-row drag is reachable at all.
 *
 * `null` for a **group header** and a **note**, and for nothing else. Those are not files: a
 * header folds and a note is a sentence, and `rowVerbs` is where that has been answered since
 * M13 — asking `kind === 'dir'` here would be the seventh site that asked and the seventh
 * meaning. Everything that *is* a file starts a drag, even when it cannot move; see `refusal`.
 *
 * All-or-nothing, the same rule *Cut* and *Move to Trash* both apply: one project root anywhere
 * in the selection refuses the whole load rather than quietly moving the other four files around
 * it. A partial drag is a gesture whose result the user cannot predict from what is highlighted.
 */
export function grab(
  /** The selection, as paths — `treeStore.selection.paths`. */
  carried: ReadonlySet<string>,
  row: DragRow,
  /** The project's roots: what may not be *moved*, and what "inside the project" means. */
  roots: readonly string[],
  /** Where cide may write — roots plus the scratch drawer, from `fs_writable_roots`. */
  writable: readonly string[],
): TreeDragSet | null {
  if (!rowVerbs(row.kind, true).actionable) return null

  // Widen only past a *multi-row* selection. A grab on the single selected row carries itself,
  // which is the same set and one less thing to explain on the ghost.
  const widen = carried.has(row.path) && carried.size > 1
  const paths = widen ? [...carried] : [row.path]
  const many = paths.length > 1

  return {
    paths,
    origin: row.path,
    label: many ? plural(paths.length) : labelFor(row),
    widened: many,
    // A root first: it is the more specific of the two and its sentence is the one the Cut menu
    // item shows for the same rows, verbatim.
    refusal: paths.some((path) => isRootPath(path, roots))
      ? 'A project root is closed, not moved'
      : mutationRefusal(paths, writable),
  }
}

/** `main.rs` for a file, `src/` for a folder — so a one-row drag says which of the two it is. */
function labelFor(row: DragRow): string {
  const name = basenameOf(row.path)
  return row.kind === 'dir' ? `${name}/` : name
}

/** What dropping a load on a row would do. `hint` is the sentence the ghost draws. */
export type TreeDropOutcome =
  | {
      readonly kind: 'move'
      /** Absolute destination directory — what `fs_paste` takes as `destDir`. */
      readonly destDir: string
      /** Only the paths that actually move; see [`dropOutcome`]. */
      readonly paths: readonly string[]
      /** The row to outline. See the field's note on the refusing variant. */
      readonly mark: string
      readonly hint: string
    }
  | { readonly kind: 'noop'; readonly destDir: string; readonly mark: string; readonly hint: string }
  | {
      readonly kind: 'refuse'
      readonly reason: string
      /**
       * The row the pointer is on, or `null` when it is on nothing.
       *
       * A refusal marks the row under the pointer rather than a destination, because there is no
       * destination — that is what the refusal says. The two accepting variants mark the
       * *destination folder* instead, which is not always the row under the pointer: dropping on
       * `src/main.rs` lands in `src`, so `src` takes the ring and its children take the band. A
       * ring on the file would claim the file was the target, which is the misreading that puts
       * a folder somewhere nobody meant.
       */
      readonly mark: string | null
      readonly hint: string
    }

function refuse(reason: string, mark: string | null): TreeDropOutcome {
  return { kind: 'refuse', reason, mark, hint: reason }
}

/**
 * The verdict for one (grab, row-under-the-pointer) pair.
 *
 * Source refusals come before target refusals, the same order `dragDrop.ts` argues for: a load
 * that can go nowhere gives one answer over every row, and saying it once teaches faster than a
 * different complaint per folder.
 *
 * One pair below is ordered and would be wrong the other way round: **outside the project before
 * the scratch drawer.** `creationRefusal` answers "not under a root" with the drawer's sentence,
 * which is right for the drawer and nonsense for `~/.cargo/registry`, so the containment test runs
 * first and only the drawer reaches it — exactly the order the context menu composes the same two
 * rules in (`rowRefusal ?? fillRefusal`). `check:tree-drag` swaps them and catches it.
 *
 * The other pair that *looks* ordered is not, and this is worth writing down because the first
 * version of this comment claimed otherwise and no mutation could break it. **"Into itself" and
 * "already there" cannot both be true.** "Already there" means every source's parent *is* the
 * destination; "into itself" means the destination is inside some source. If the destination is
 * `S`'s parent it cannot also be inside `S`, so the two are exclusive by construction rather than
 * by ordering, for one source and for any number of them. The refusal is written first anyway,
 * because it is the one whose being missed destroys a directory tree and it should be the first
 * thing a reader meets — but nothing depends on that, and a check script asserting it would be
 * asserting a state that cannot exist.
 */
export function dropOutcome(
  drag: TreeDragSet,
  /** The row under the pointer, or `null` for empty space and for anywhere outside the tree. */
  target: DragRow | null,
  roots: readonly string[],
  writable: readonly string[],
): TreeDropOutcome {
  if (drag.refusal !== null) return refuse(drag.refusal, target?.path ?? null)

  /*
   * Nothing under the pointer is a refusal, **not** a drop into the project root.
   *
   * `targetFor(null, roots)` answers `roots[0]`, which is right for *New File…* from the empty
   * space under the last row and would be dangerous here: a pointer over the editor, over the
   * terminal, or past the bottom of the panel resolves to no row too, so "no row means the
   * project root" would turn every abandoned drag released outside the tree into a move to the
   * top of the project. `dragDrop.ts` refuses the same case for the same reason.
   */
  if (target === null) return refuse('Drop on a folder in the project', null)

  // A group header or a note. Its `path` is a `cide://…` sentinel, so `targetFor` below would
  // happily hand back the sentinel's "parent" and Rust would refuse a path that means nothing.
  // Refused here instead, in words that name what the row is.
  if (isSyntheticPath(target.path)) {
    return refuse(
      target.kind === 'group'
        ? `“${target.name}” is a heading, not a folder`
        : 'That row is a message, not a folder',
      target.path,
    )
  }

  // The same call *New File…* and *Paste* make, so all three land in the same directory by
  // construction: a folder row is itself, a file row is its parent.
  const dest = targetFor({ path: target.path, isDir: target.kind === 'dir' }, roots)
  if (dest === null) return refuse('This project has no folder to drop into', target.path)

  /*
   * Outside everywhere cide may write — every row under *External Libraries*.
   *
   * `fs_paste` refuses this in `check_dest` → `ops::check_within`, and a rejected command with
   * `~/.cargo/registry/src/…` in it is not an answer anybody can act on. `writable` is the list
   * Rust checks against, delivered by `fs_writable_roots`, and not the roots: the *Scratches*
   * drawer is outside every root and is genuinely writable, so deriving this from a second copy
   * of the rule is precisely how the two would come apart.
   */
  if (rootOf(dest.parent, writable) === null) {
    return refuse(`“${dest.label}” is read-only — it is outside the project`, dest.parent)
  }

  /*
   * Writable, and still not a folder anybody fills: the scratch drawer.
   *
   * Rust would accept this one. It is refused because the menu refuses it — *New File in…* and
   * *Paste* are both off inside the drawer — and two gestures that answer the same question
   * differently is how a user learns the app is guessing. `creationRefusal` is that rule, called
   * rather than restated.
   */
  const fill = creationRefusal(dest.parent, roots)
  if (fill !== null) return refuse(fill, dest.parent)

  /*
   * A folder into itself, or into anything under it. **The one that destroys a tree.**
   *
   * `cide_fs::copy::check_source` refuses it too (`FsError::IntoItself`), and this copy is not
   * belt and braces for its own sake: without it the gesture is drawn as an ordinary accepting
   * drop, right up to the moment the user lets go of a directory over one of its own children.
   * `isInside` is component-wise, so `/w/srcx` is not inside `/w/src` — the string version of
   * this test refuses a perfectly good drop into a sibling whose name shares a prefix.
   */
  const itself = drag.paths.find((path) => isInside(dest.parent, path))
  if (itself !== undefined) {
    return refuse(`“${basenameOf(itself)}” cannot be moved into itself`, dest.parent)
  }

  /*
   * Only what actually moves.
   *
   * `cide_fs::copy` already treats a cut into the folder a file is already in as a no-op rather
   * than a duplicate, so sending everything would be correct — but a drop that reports success
   * and changes nothing on screen reads as a broken drag, and a drop of five files where two are
   * already there should say it moved three. Same discipline as `dragDrop.ts::dropOutcome`.
   */
  const moving = drag.paths.filter((path) => dirnameOf(path) !== dest.parent)
  if (moving.length === 0) {
    return {
      kind: 'noop',
      destDir: dest.parent,
      mark: dest.parent,
      hint: `Already in “${dest.label}”`,
    }
  }
  const what = moving.length === 1 ? `“${basenameOf(moving[0] ?? '')}”` : plural(moving.length)
  return {
    kind: 'move',
    destDir: dest.parent,
    paths: moving,
    mark: dest.parent,
    hint: `Move ${what} to “${dest.label}”`,
  }
}

/**
 * Is this row part of the load in flight?
 *
 * Used to dim the source rows, and it is not plain set membership: dragging `src/` puts every row
 * under it in flight too, and drawing those as untouched would say the files inside the folder are
 * staying where they are.
 *
 * Walks the row's **ancestors** and asks the set, rather than scanning the load and asking
 * `isInside` per member. The two answer the same question and the costs are not comparable: this
 * is called for every visible row on every pointer move, and Ctrl+A caps a selection at
 * `RANGE_ROWS` — 10 000 — so the scan would be 300 000 prefix comparisons per frame against a
 * path's ~20 components here. The set is built once per drag by the caller for the same reason.
 *
 * `cut > 0` and not `>= 0`: the ancestor chain stops at `/`, which is never in a drag load and
 * would be tested as the empty string.
 */
export function inDrag(paths: ReadonlySet<string>, rowPath: string): boolean {
  if (paths.has(rowPath)) return true
  for (let cut = rowPath.lastIndexOf('/'); cut > 0; cut = rowPath.lastIndexOf('/', cut - 1)) {
    if (paths.has(rowPath.slice(0, cut))) return true
  }
  return false
}

/**
 * Is this row inside the folder the drop would land in?
 *
 * The tint that says "this folder", drawn on the destination's children as well as on its own
 * row. Not decoration: the destination is very often *above the fold* — drop onto a file 30 rows
 * into an expanded folder and the folder's own row is off the top of a 252px panel — so a ring
 * on the header alone would highlight nothing at all, which is the state in which people let go
 * over the wrong folder. `GitPanel/dragDrop.ts::bands` exists for the identical reason.
 *
 * A predicate rather than that module's precomputed map, because paths carry their own ancestry:
 * a changelist has to be found by walking back up the row list, a directory does not.
 */
export function inDropBand(rowPath: string, destDir: string): boolean {
  return rowPath !== destDir && isInside(rowPath, destDir)
}

/**
 * The folder a resting pointer should unfold, or `null`.
 *
 * Spring-loaded folders: you cannot drop into a folder you cannot see, and the alternative —
 * abandon the drag, expand the folder, start again — is the gesture people give up on. The
 * *timing* is the hook's; which rows qualify is a decision and lives here.
 *
 * Three conditions, and each rules out a real accident:
 *
 * * **a collapsed directory with children.** An empty folder has no twisty and unfolding it
 *   moves nothing on screen, so the timer would fire for ever with no visible effect.
 * * **not a refused drop.** This is the load-bearing one. A group header is expandable, and
 *   expanding *External Libraries* is what launches `cargo metadata` — so without this, resting
 *   the pointer over a heading the drop can never land on would spawn a toolchain. `kind === 'dir'`
 *   already excludes the header itself; the guard also covers a dependency directory under it,
 *   whose expansion is a `read_dir` in somebody else's tree.
 * * **the row under the pointer**, not the destination. A file row's destination is its parent,
 *   which is by definition already open.
 */
export function springTarget(row: DragRow | null, outcome: TreeDropOutcome): string | null {
  if (row === null || outcome.kind === 'refuse') return null
  if (row.kind !== 'dir' || row.expanded || !row.hasChildren) return null
  return row.path
}
