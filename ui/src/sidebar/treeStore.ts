/**
 * The file tree's windowed row cache.
 *
 * Rust owns the tree; this owns a bounded view of it. Three rules the M8 criteria turn on:
 *
 * 1. Rows are fetched in fixed-size chunks (see `rowWindow.ts`), never in whatever range the
 *    virtualizer happens to be showing, so requests deduplicate.
 * 2. Resident chunks are capped, so the JS heap stays flat regardless of repository size —
 *    scrolling a 100k-file tree end to end must not retain every row it passed.
 * 3. A structural change the *user* made — expand, collapse, reveal — drops the whole cache.
 *    A partial invalidation would need to know which rows moved, and the flattened tree
 *    renumbers everything after the change; a cache that is subtly wrong about row 900 shows
 *    the user a file that is not there.
 *
 * # A watcher burst is a revalidation, not an invalidation
 *
 * Rule 3 used to cover `cide://fs-changed` as well, and that is what made the tree flicker.
 * The burst arrives, [`refresh`] empties `chunks`, and every visible row's `rowAt` answers
 * `undefined` until `fs_tree_rows` comes back — so the tree blanks to placeholders and
 * refills, once per burst, whether or not a single row moved. It usually has not: `.git/HEAD`,
 * `.git/index` and the refs are watched deliberately (`cide_fs::filter::GIT_WATCHED`) so the
 * git tags stay true, and `Index::apply` skips those paths *by name* because they can change
 * no row. Every git command in a terminal pane therefore repainted the whole tree from empty.
 *
 * So [`refresh`] re-reads the count and the chunks the viewport is showing, compares them
 * against what is cached, and **writes nothing at all when nothing moved** — zero re-renders
 * for a burst the tree does not care about. When something did move it writes once, with the
 * fresh rows already in it, so no frame is ever short a row. Chunks outside the viewport are
 * not re-read on every burst; they are marked stale in module scope (no re-render) and
 * revalidated when they are next scrolled into view, still showing their old rows until the
 * new ones land.
 *
 * Everything reaches Rust through [`tree`] below, which layers two different kinds of "no
 * answer" on top of each other: a handler that is not in this build at all (`pendingCommand`
 * — an empty tree and one log line, never a rejected promise inside a render), and a project
 * whose walk has not started yet (`NoIndex` — an empty tree and nothing else, because it is
 * about to fill in). Conflating them is what made a freshly opened project claim the build
 * was missing its file commands.
 */
import { create } from 'zustand'
import {
  fs as fsApi,
  fsCreate,
  isDegraded,
  pendingCommand,
  type ProjectId,
  type TreeRow,
} from '@/ipc/client'
import { isNoIndex } from '@/store/fileIndex'
import { groupPath, rowVerbs } from './groupRows'
import { CHUNK_CAP, CHUNK_ROWS, chunkOf, chunkRequest, chunksFor, chunksToEvict } from './rowWindow'
import {
  applyRange,
  collapseTo,
  keySelect,
  NO_PATHS,
  pressSelect,
  rangeRefusal,
  releaseSelect,
  type SelectMods,
  type SelectPlan,
  type TreeSelection,
} from './treeSelection'

/**
 * The writable set before Rust has answered, shared so `attach` and the initial state are the
 * same object — a fresh `[]` per reset would re-render every subscriber for no change.
 */
const NO_WRITABLE: readonly string[] = []

/** The same, for the reveal set. Separate constant, separate identity, same argument. */
const NO_REVEALABLE: readonly string[] = []

/**
 * A tree command, with "the index is not built yet" separated from "there is no such
 * handler".
 *
 * The store attaches to a project the moment it becomes active, which is *before*
 * `store/workspace.ts` has finished asking Rust to walk it — necessarily so, since the walk
 * takes seconds on a large repository and the panel is not going to wait. Every call in that
 * window is answered `NoIndex`, and routing that through `pendingCommand` alone latched
 * `degraded`, whose notice reads "not registered in this build". Permanently: the flag never
 * clears. So the honest answer for `NoIndex` is the fallback with no flag raised, and the
 * `cide://fs-status` the walk emits at its end is what brings the rows in — see `Explorer`.
 */
async function tree<T>(name: string, call: () => Promise<T>, fallback: T): Promise<T> {
  return pendingCommand(
    name,
    async () => {
      try {
        return await call()
      } catch (error) {
        if (isNoIndex(error)) return fallback
        throw error
      }
    },
    fallback,
  )
}

/**
 * What a press answered: the half the caller needs now, and the half it can wait for.
 *
 * See `FileTreeStore.pressRow` for why the deferral cannot travel inside the promise.
 */
export interface TreePress {
  /** The collapse is waiting for the release — this press may be the start of a drag. */
  readonly deferred: boolean
  /** The refusal sentence for a band that was too wide, or `null`. */
  readonly settled: Promise<string | null>
}

interface FileTreeStore {
  project: ProjectId | null
  /** Total rows in the flattened tree. The virtualizer's `count`. */
  count: number
  /** Resident rows, keyed by chunk index. */
  chunks: Map<number, TreeRow[]>
  /**
   * True once an `fs_*` call has failed for a reason other than "not indexed yet", i.e. the
   * commands are not in this build. A project waiting for its first walk is not degraded.
   */
  degraded: boolean
  /**
   * Every directory this project's disk-changing verbs may act inside: its roots, **plus** the
   * scratch drawer.
   *
   * Not derivable here. The drawer is `$XDG_STATE_HOME/cide/scratches/<blake3 of the
   * canonicalised primary root>` — a fact only Rust can compute — and it is outside every root
   * by construction, which is the whole point of a scratch. So it is asked once per attach,
   * from `fs_writable_roots`, which answers with the *same list* `cide_fs::ops::check_within`
   * is given. Two derivations of "may cide write here" is how a menu comes to offer a verb the
   * handler then refuses, which is the dead control this panel has already shipped twice.
   *
   * `[]` until the answer lands, and `FileTree` unions it with the workspace mirror's roots
   * rather than using it alone — so for that one frame a project file is still mutable and a
   * scratch is not, which is the safe direction of being briefly wrong.
   */
  writable: readonly string[]
  /**
   * Every path the tree can hang a row from: the roots, **plus** each group's top-level children
   * — a package directory under *External Libraries*, a file in the *Scratches* drawer. (M16)
   *
   * Not the writable set and not derivable from it: a dependency source is revealable and must
   * never be writable, and the scratch *drawer* is writable while drawing no row of its own. Two
   * questions, two lists, from the two Rust functions that answer them.
   *
   * The reader is `chrome/StatusBar.tsx` through `rowPaths::crumbTargets`, which is why this
   * lives in the file tree's store rather than beside the readout: it is a fact about the tree,
   * and the tree is the thing a crumb reveals *into*.
   *
   * `[]` until the answer lands, which makes every crumb inert for that frame — the safe
   * direction, since the alternative is a live-looking crumb that reports itself. It is re-asked
   * on the self-heal below rather than once, because unlike `writable` this answer genuinely
   * moves a second time: *External Libraries* resolves on a background thread and emits
   * `cide://fs-status` when it lands, and that is the burst that turns the library crumbs on.
   */
  revealable: readonly string[]
  /**
   * A row index the tree should scroll to, set by `reveal` and cleared by the component
   * once it has scrolled. A number rather than a boolean flag so two reveals in a row are
   * two scrolls.
   */
  revealTo: number | null
  /**
   * The selected row, by **path**.
   *
   * Selection is now a first-class thing, distinct from what is open: a single click selects
   * and only a double click opens. Which means the selection has to survive the thing that
   * happens to this tree constantly — a watcher burst re-reading the rows underneath it. A
   * row *index* does not survive that (a file created above the selection renumbers it), and
   * a selection that jumps to a different file on every `fs-changed` is worse than none at
   * all. A path does survive it, and is also what every action a menu offers actually needs.
   *
   * It lives in the store rather than in `FileTree`'s own `useState` for the same reason:
   * the panel unmounts every time the user clicks another icon in the activity rail, and a
   * selection that is lost by looking at Search is a selection nobody can rely on.
   */
  selected: string | null
  /**
   * Where the selected row last was. Advisory, and clamped by every reader.
   *
   * The arrows need a *position* to move from and the selection is an identity, so one of the
   * two has to be derived. Deriving the index is cheap and correct when the row is resident
   * ([`indexOf`]); this is the fallback for when it is not — scrolled far out of the cache,
   * or inside a collapsed folder — where the honest answer is "about here".
   */
  selectedIndex: number
  /**
   * Every selected row, and where a Shift-range starts from. The rules are `treeSelection.ts`.
   *
   * A **second** concept beside [`selected`], not a replacement for it, and the git panel keeps
   * the same two apart for the same reason (`rowSelection.ts`, `useGitPanel::current`):
   *
   * | | what it means |
   * | --- | --- |
   * | [`selected`] | the cursor — where the arrows move from, and the row Rename, Open and Reveal act on |
   * | `selection` | what is highlighted, and what Cut, Copy and Move to Trash act on |
   *
   * They coincide for a plain click, which is why one field did for as long as a selection was
   * one row. Ctrl-clicking a highlighted row off is where they come apart: the cursor stays
   * where the gesture landed and the row is no longer in `selection`, so a cursor that doubled
   * as the scope would delete a file with nothing on screen marking it.
   */
  selection: TreeSelection
  /**
   * The unnamed row the user is typing a new file or folder into, or `null`.
   *
   * **An inline editor row, not a dialog**, and the choice is worth stating because the
   * virtualized list is what makes it work rather than what makes it hard. A modal would be
   * the smaller change — one `<input>`, no row arithmetic — and it would cover the very list
   * that gives the name its context: the panel is 252px wide, the thing being named is one
   * word, and the question the user is actually answering is *"beside what?"*. Every editor
   * puts the box in the tree for that reason, and this tree already has the machinery from
   * the rename box.
   *
   * The row does not exist in Rust. It is drawn *between* two real rows by `FileTree`, which
   * renders `count + 1` rows and shifts every index at or past `index` by one — which is also
   * why this is a flattened index rather than a parent-plus-offset: the virtualizer addresses
   * positions, and a position is the only thing the draft and the real rows have in common.
   */
  draft: {
    /** The directory the entry will be created in. Absolute. */
    parent: string
    /** Folder rather than file. Chooses the icon, the placeholder and the command argument. */
    directory: boolean
    /** The flattened row the editor occupies. Rows from here on are shifted down by one. */
    index: number
    /**
     * How far to indent it: the parent's depth plus one.
     *
     * Carried here rather than re-derived from the row above at render time. That row is not
     * resident at the moment the draft opens — `beginDraft` has just re-flattened the tree and
     * emptied the cache — so the renderer's first frame would indent the box at depth 0 and
     * jog it sideways when the chunk landed. Counting separators in `parent` is not the
     * alternative either: that is depth in the *filesystem*, which a multi-root project makes
     * a different number from depth in the flattening.
     */
    depth: number
    /** Names already in `parent`, for the "already exists" check while typing. */
    siblings: string[]
  } | null

  /** Point the tree at a project, or at nothing. Reads the row count. */
  attach: (project: ProjectId | null) => Promise<void>
  /**
   * Ask for whatever chunks cover `[from, to)`, and record that range as the visible one.
   * Cheap and idempotent; call from an effect on every render.
   */
  ensure: (from: number, to: number) => void
  /** The row at a flattened index, or `undefined` while its chunk is in flight. */
  rowAt: (index: number) => TreeRow | undefined
  /**
   * Where a path currently sits, or `null` if no resident chunk holds it.
   *
   * Only resident rows are searched — this is a scan of what is already in memory, never a
   * round trip. `null` therefore means "not on screen and not nearby", which is exactly the
   * case in which `selectedIndex` is the better answer, not a case worth an IPC call for.
   */
  indexOf: (path: string) => number | null
  /**
   * Select exactly one row, collapsing whatever else was selected. `index` is a hint for the
   * arrows; see `selectedIndex`.
   *
   * The unmodified gesture, and still the one most callers want: a reveal, a paste landing, a
   * freshly created file. [`pressRow`] is the one that reads Ctrl and Shift.
   */
  select: (path: string, index: number) => void
  /**
   * A left press on a row, with modifiers — the mouse half of the multi-selection.
   *
   * Answers **synchronously** with whether the collapse was deferred, and asynchronously (in
   * `settled`) with a sentence for the problem strip when the gesture was refused — a Shift-range
   * wider than `RANGE_ROWS` — or `null` when it was carried out. A band whose rows are not all
   * resident has to be asked for; the *cursor* moves before the await either way, so nothing
   * about it waits on IPC.
   *
   * The two halves are split rather than folded into one promise because the caller has to know
   * about the deferral *at press time*, to remember the row whose release will finish the job.
   * Reading it out of a promise would put a rule about mousedown behind a microtask that a fast
   * click can, in principle, outrun — and a selection that collapses only sometimes is worse than
   * one that never does. See `TreePressPlan`.
   */
  pressRow: (path: string, index: number, mods: SelectMods) => TreePress
  /**
   * The release of a press [`pressRow`] deferred: collapse to that row after all.
   *
   * The caller runs it only when the gesture did **not** become a drag; see `useTreeDrag`.
   */
  releaseRow: (path: string, index: number) => void
  /** The same as [`pressRow`], for an arrow that has already decided which row it is moving to. */
  keyToRow: (path: string, index: number, mods: SelectMods) => Promise<string | null>
  /** Write a selection worked out elsewhere — the right-click rule; see `pressMenu`. */
  setSelection: (next: TreeSelection, lead: string | null, index: number) => void
  /** Nothing selected, and no cursor. What a delete leaves behind. */
  clearSelection: () => void
  /**
   * Ctrl+A: every row in the flattened tree, or a refusal when there are too many.
   *
   * Every row, not every *visible* one — unlike the git panel's, whose `rows` array only ever
   * holds what is on screen. A folded folder here contributes no row at all, so this is already
   * "what the tree is showing" in the only sense the flattening has. The cursor is left where
   * it is: Ctrl+A changes what is selected, not where you are.
   */
  selectAll: () => Promise<string | null>
  /** Expand or collapse a directory row. */
  toggle: (row: TreeRow) => Promise<void>
  /**
   * Expand ancestors until `path` is visible, then scroll to it. `false` when there is no row.
   *
   * The answer is not decoration. `fs_reveal` is index-only, so it says `null` for a
   * gitignored file, a file deleted between the pick and the reveal, and — since M13 — for the
   * ordinary case of a tab holding a file that is not in the project at all. This store used to
   * swallow all three, which made *Reveal in Files* a listed, enabled, keyboard-reachable
   * command that silently did nothing; the caller now has something to say a sentence about.
   */
  reveal: (path: string) => Promise<boolean>
  /**
   * Scroll to a synthetic group's header, expand it, and select it. `false` when this project
   * has no such group.
   *
   * A separate action from [`reveal`] because the two differ in the step that matters: this one
   * **expands the row it lands on**, which is also what starts the dependency resolution. A
   * reveal deliberately does not — unfolding a directory somebody only asked to be shown moves
   * every row below it — so folding the expand into `reveal` would have changed that for every
   * caller to give one command what it needs.
   *
   * `false` rather than a silent no-op is the whole point: a project with no `Cargo.toml` and no
   * `go.mod` has no group row at all, and a palette entry that scrolls nowhere and says nothing
   * is the defect this codebase keeps finding.
   */
  revealGroup: (id: string) => Promise<boolean>
  clearReveal: () => void
  /**
   * Re-read the count and the visible rows, and update only if they moved. The
   * `cide://fs-changed` and `cide://fs-status` handler; see the module header.
   */
  refresh: () => Promise<void>

  /**
   * Open the inline editor for a new entry inside `parent`, and scroll it into view.
   *
   * `atTop` is the single-root project's hidden root — see `newEntry.ts`. Everything else is
   * anchored under the parent's own row, which means the parent has to be *expanded* and its
   * row index has to be known, in that order:
   *
   * 1. `fs_reveal` expands every ancestor and answers the parent's row. Reveal first, because
   *    expanding an ancestor is what moves the parent's own row number.
   * 2. `fs_expand` on the parent itself. Expansion only adds rows *below* the parent, so the
   *    index from step 1 survives it — which is why it is not re-read.
   *
   * The draft then sits at `parent + 1`: the parent's first child position. Two round trips
   * for one menu click, which is the right trade — the alternative is drawing the editor
   * somewhere the user is not looking.
   *
   * `false` when there is nowhere to draw it: the parent has no row, because it is gitignored
   * or because it was deleted between the right-click and now. The caller says so — a menu
   * item that opens no box and reports nothing is the dead control this codebase keeps
   * shipping.
   */
  beginDraft: (parent: string, directory: boolean, atTop: boolean) => Promise<boolean>
  /** Abandon the draft. Escape, blur with nothing typed, and every project switch. */
  cancelDraft: () => void
  /**
   * Create what the draft describes and select the new row. Rejects on refusal.
   *
   * Resolves to the created path when the tree ended up with a row for it, and to `null` when
   * it did not — a name the project's ignore rules hide is genuinely created and genuinely has
   * no row, and the caller has to be able to say so rather than leave the user looking for it.
   *
   * The new row is **selected and not opened**. Creating a file is not opening it; that is the
   * rule the single-click change established, applied to creation.
   */
  commitDraft: (name: string) => Promise<string | null>
}

/**
 * Chunks already requested, whether or not they have landed.
 *
 * Module scope rather than store state on purpose: it is bookkeeping the renderer must never
 * read, and putting it in the store would make every in-flight request a state update and so
 * a re-render of the whole tree. `touched` is the LRU order behind `CHUNK_CAP`.
 */
let inFlight = new Set<number>()
let touched: number[] = []
/**
 * The chunks the last `ensure` asked for — what the user can currently see.
 *
 * Module scope for the same reason as `touched`, and load-bearing for `refresh`: a burst
 * re-reads the viewport and nothing else, so its cost is proportional to the panel rather
 * than to `CHUNK_CAP`.
 */
let visible = new Set<number>()
/**
 * Resident chunks that may no longer describe the tree.
 *
 * Deliberately *not* an eviction. A stale chunk keeps its rows on screen and is re-requested
 * the next time `ensure` reaches it; `loadChunk` then overwrites them if they moved and
 * writes nothing if they did not. Dropping them instead is the one-frame blank this whole
 * file is about, moved from the burst to the scroll.
 */
let stale = new Set<number>()
/**
 * Bumped on every invalidation. A response tagged with an older generation is dropped —
 * without this, a `fs_tree_rows` reply that was in flight when the user collapsed a folder
 * lands afterwards and writes rows from the *previous* flattening into the new one.
 */
let generation = 0

function resetCache(): void {
  inFlight = new Set()
  touched = []
  visible = new Set()
  stale = new Set()
  generation += 1
}

/**
 * Invalidate from `chunk` onward, keeping the module state of the chunks strictly below it.
 *
 * The fold case: expanding or collapsing the row at flat index `i` re-flattens everything
 * *after* `i` and moves nothing before it, so the chunks above the fold still describe the
 * tree exactly. Dropping them anyway — which `toggle` did with a plain `resetCache()` — is
 * the one-frame blank of the whole visible viewport on every fold, the exact flicker this
 * module's header is about, delivered by the gesture users make most.
 *
 * `stale` and `touched` are filtered rather than cleared: a retained chunk that a watcher
 * burst had already marked stale must stay stale, or its re-request is silently forgotten
 * and it draws old rows for ever. The generation still moves — an in-flight reply describes
 * the previous flattening even for a low chunk, and `loadChunk` bails on the check.
 */
function retainCacheBelow(chunk: number): void {
  inFlight = new Set()
  visible = new Set()
  touched = touched.filter((c) => c < chunk)
  stale = new Set([...stale].filter((c) => c < chunk))
  generation += 1
}

/**
 * Every field of `TreeRow`, named where the compiler checks the list.
 *
 * `sameRows` below is what decides that a burst moved nothing and that the tree therefore
 * must not re-render. A hand-written chain of `a.path === b.path && …` was the first version
 * and it is the fragile one: `TreeRow` is generated from Rust, so a field added there is
 * simply absent from the chain, `sameRows` answers "identical" for two rows that differ, and
 * the tree keeps drawing the old value with no re-render left to correct it — silent
 * staleness, which is worse than the flicker this file is about. `Record<keyof TreeRow, true>`
 * turns that into a compile error the moment `pnpm codegen` adds the field.
 */
const ROW_FIELDS: Readonly<Record<keyof TreeRow, true>> = {
  path: true,
  name: true,
  depth: true,
  kind: true,
  expanded: true,
  hasChildren: true,
  symlink: true,
  root: true,
  // M13. The group header's count and a dependency's version live here, and both change
  // *without any row moving*: `External Libraries` goes from no detail to `593` when a
  // resolution lands, on the same row, at the same index. Without this entry `sameRows` would
  // call the two identical and the count would never appear — the exact silent staleness the
  // compiler-checked census exists to prevent, arriving in the same batch that added the field.
  detail: true,
}
const ROW_KEYS = Object.keys(ROW_FIELDS) as Array<keyof TreeRow>

/**
 * Whether two lists of paths are the same list.
 *
 * For the reveal set, which is a *prop of the status bar*: writing back a fresh array of equal
 * strings would re-render the bar and re-run its crumb classification on every watcher burst that
 * happened to add a file.
 */
function sameList(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((path, i) => path === b[i])
}

/** Whether two windows of rows describe the same thing. Field-wise: `TreeRow` is flat. */
function sameRows(a: readonly TreeRow[] | undefined, b: readonly TreeRow[]): boolean {
  if (a === undefined || a.length !== b.length) return false
  return a.every((row, i) => {
    const other = b[i]
    return other !== undefined && ROW_KEYS.every((key) => row[key] === other[key])
  })
}

/** One chunk's rows, or `[]` for a chunk that is entirely past the end. */
function readChunk(project: ProjectId, chunk: number, total: number): Promise<TreeRow[]> {
  const { offset, len } = chunkRequest(chunk, total)
  if (len <= 0) return Promise.resolve([])
  return tree('fs_tree_rows', () => fsApi.treeRows(project, offset, len), [] as TreeRow[])
}

export const useFileTree = create<FileTreeStore>((set, get) => ({
  project: null,
  count: 0,
  chunks: new Map(),
  degraded: false,
  writable: NO_WRITABLE,
  revealable: NO_REVEALABLE,
  revealTo: null,
  selected: null,
  selectedIndex: 0,
  selection: NO_PATHS,
  draft: null,

  async attach(project) {
    // A remount over the SAME project — the rail toggling Files shut and back open, or a
    // panel switch away and back — must not blank a tree the user was just looking at:
    // the reset below zeroes the count, so the virtualizer draws nothing until three
    // sequential round trips resolve. `refresh()` is already the no-flicker revalidation
    // (it re-reads the count and the visible rows, writes only what moved, and self-heals
    // the writable and revealable sets), so a reattach *is* a refresh. The full reset is
    // for a genuinely different project — and a caller that someday reuses a project id
    // across a roots change must add an explicit reset flag here; today no such caller
    // exists (`Explorer`'s effect keys on the id).
    if (project !== null && get().project === project) return get().refresh()
    resetCache()
    // The selection goes with the project. A path from the old one names nothing in the new
    // tree, and keeping it would leave a highlight the user cannot see and the arrows
    // starting from a row that does not exist.
    //
    // So does the draft, and for a sharper reason: its `parent` is an absolute path in the
    // project being left, so a draft that survived the switch would create a file in a
    // project the user is no longer looking at.
    set({
      project,
      count: 0,
      chunks: new Map(),
      revealTo: null,
      selected: null,
      selectedIndex: 0,
      selection: NO_PATHS,
      draft: null,
      // The old project's drawer names nothing in the new one, and leaving it would make a
      // stale path look mutable for as long as the fetch below takes.
      writable: NO_WRITABLE,
      // Same argument one step over: a crumb of the *previous* project's trail would keep
      // testing as revealable, and the click would land on a tree that no longer holds it.
      revealable: NO_REVEALABLE,
    })
    if (project === null) return

    const mine = generation
    const count = await tree('fs_tree_count', () => fsApi.treeCount(project), 0)
    if (generation !== mine) return
    set({ count, degraded: isDegraded('fs_tree_count') })

    /*
     * The writable set, after the count and not awaited alongside it.
     *
     * The rows are what the user is waiting for; this only decides whether five context-menu
     * items are greyed, and a tree that painted a frame later so a menu could be right would be
     * the wrong trade. Its own `tree()` fallback, so a build whose backend predates
     * `fs_writable_roots` keeps a working file tree and merely refuses to rename a scratch —
     * which it also has no way to create.
     */
    const writable = await tree('fs_writable_roots', () => fsApi.writableRoots(project), [])
    if (generation !== mine || get().project !== project) return
    set({ writable })

    /*
     * And the reveal set, on the same terms and for the same reason.
     *
     * Sequential rather than `Promise.all`ed with the one above: both are `blocking()` calls onto
     * the same pool behind the same project lock, so racing them buys nothing, and this one is
     * strictly less urgent — it decides whether a *status bar* crumb is drawn live.
     *
     * `fs_reveal_roots` never resolves *External Libraries*, so on a cold project this answers
     * with the roots alone. That is why `refresh` asks again; see the self-heal there.
     */
    const revealable = await tree('fs_reveal_roots', () => fsApi.revealRoots(project), [])
    if (generation !== mine || get().project !== project) return
    set({ revealable })
  },

  ensure(from, to) {
    const { project, count, chunks } = get()
    if (project === null || count === 0) return

    const wanted = chunksFor(Math.max(0, from), Math.min(to, count), CHUNK_ROWS)
    visible = new Set(wanted)
    for (const chunk of wanted) {
      // Touch first, so a chunk being re-visited moves to the back of the eviction queue
      // even when it is already resident and no request is made.
      const at = touched.indexOf(chunk)
      if (at >= 0) touched.splice(at, 1)
      touched.push(chunk)

      if (inFlight.has(chunk)) continue
      // A stale chunk is re-requested but not dropped: its rows are what the user is looking
      // at, and blanking them for the length of a round trip is the flicker. `loadChunk`
      // replaces them only if they actually moved.
      if (chunks.has(chunk) && !stale.has(chunk)) continue
      inFlight.add(chunk)
      void loadChunk(project, chunk, new Set(wanted), set, get)
    }
  },

  rowAt(index) {
    const chunk = get().chunks.get(chunkOf(index))
    return chunk?.[index % CHUNK_ROWS]
  },

  indexOf(path) {
    for (const [chunk, rows] of get().chunks) {
      const at = rows.findIndex((row) => row.path === path)
      if (at >= 0) return chunk * CHUNK_ROWS + at
    }
    return null
  },

  select(path, index) {
    const { selected, selectedIndex, selection } = get()
    // Guarded because a click on the already-selected row is the common case (it is the first
    // half of every double-click), and an unconditional `set` would re-render every visible
    // row to draw exactly what it is drawing.
    //
    // The selection is part of the test and not an afterthought: a plain click on the row the
    // cursor is already on has to *collapse* a multi-row selection built around it, and the
    // two-field version of this guard bailed out before it could — the highlight stayed on
    // five rows after a click that meant "just this one".
    const alone = selection.paths.size === 1 && selection.paths.has(path)
    if (selected === path && selectedIndex === index && alone) return
    set({ selected: path, selectedIndex: index, selection: collapseTo(path) })
  },

  pressRow(path, index, mods) {
    const { plan, deferred } = pressSelect(get().selection, path, mods)
    return { deferred, settled: runPlan(plan, path, index, set, get) }
  },

  releaseRow(path, index) {
    set({ selection: releaseSelect(path), selected: path, selectedIndex: index })
  },

  async keyToRow(path, index, mods) {
    return runPlan(keySelect(get().selection, path, mods), path, index, set, get)
  },

  setSelection(next, lead, index) {
    set({ selection: next, selected: lead, selectedIndex: index })
  },

  clearSelection() {
    const { selection, selected } = get()
    if (selection.paths.size === 0 && selected === null) return
    set({ selection: NO_PATHS, selected: null, selectedIndex: 0 })
  },

  async selectAll() {
    const { count } = get()
    if (count === 0) return null
    const refusal = rangeRefusal(count)
    if (refusal !== null) return refusal
    const before = get().selection
    const paths = await bandPaths(0, count - 1, get)
    // The identity check is the whole guard against a stale write: the fetch above can take a
    // frame or two, and anything the user did to the selection in between — a click, an arrow,
    // an Escape — must outrank a Ctrl+A they have already moved on from.
    if (paths === null || paths.length === 0 || get().selection !== before) return null
    set({ selection: { paths: new Set(paths), anchor: paths[0] ?? null } })
    return null
  },

  async toggle(row) {
    const { project } = get()
    // `rowVerbs`, not `kind === 'dir'`: a group header folds too, and a note must not. This is
    // one of the six sites that used to ask about `'dir'` and meant six different things.
    if (project === null || !rowVerbs(row.kind, true).expandable) return

    const call = row.expanded
      ? () => fsApi.collapse(project, row.path)
      : () => fsApi.expand(project, row.path)
    const name = row.expanded ? 'fs_collapse' : 'fs_expand'

    // The toggled row's flat index, read from the resident cache *before* the round trip:
    // rows strictly above the fold cannot move when a subtree folds or unfolds, so their
    // chunks survive the invalidation and the viewport above the fold keeps its rows on
    // screen for the length of the trip. `null` — the row is somehow not resident — falls
    // back to keeping nothing, which is the old full reset.
    const at = get().indexOf(row.path)

    // The count comes back from the same call rather than a follow-up `fs_tree_count`: the
    // two would be separate round trips against a tree that another expand could change
    // between them, and the virtualizer would briefly size itself to a count that never
    // matched the rows.
    const count = await tree(name, call, get().count)
    // The toggled row's own chunk is dropped with everything after it — its `expanded` flag
    // changed, so its rows genuinely differ.
    const keepBelow = at === null ? 0 : chunkOf(at)
    retainCacheBelow(keepBelow)
    const kept = new Map([...get().chunks].filter(([index]) => index < keepBelow))
    set({ count, chunks: kept, degraded: isDegraded(name) })
  },

  async reveal(path) {
    const { project } = get()
    if (project === null) return false

    // `fs_reveal` answers `null` for a path outside the index — a gitignored file, a stale
    // picker row, a file deleted between the pick and the reveal, or a tab holding a file the
    // project does not contain at all. Treated as "nothing to scroll to" rather than coerced to
    // a row number, and *reported*: the caller is the one that knows whether silence is honest.
    const index = await tree('fs_reveal', () => fsApi.reveal(project, path), null)
    // The project can be swapped out from under a reveal — `attach` is what a project switch
    // runs — and writing this count and this index into the new project's tree would scroll it
    // to a row belonging to the old one.
    if (index === null || index < 0 || get().project !== project) return false

    // Revealing expands ancestors, so the flattening moved; everything cached is stale.
    resetCache()
    const mine = generation
    const count = await tree('fs_tree_count', () => fsApi.treeCount(project), get().count)
    // The same guard `attach` and `refresh` carry, for the same reason: an expand or a watcher
    // refresh that landed while this count was in flight has already re-flattened the tree, so
    // both the count and the row index below describe a shape that is gone.
    // `true`, not `false`: the row exists and the ancestors are expanded — the tree simply
    // re-flattened under the count, and the next `refresh` puts it on screen. Answering `false`
    // here would make the caller say "that file is not in this project" about a file that is.
    if (generation !== mine) return true
    // A reveal scrolls to a row the user was not looking at, so it also selects it: landing
    // on a screenful of rows with nothing marked leaves them to find the file again by eye,
    // which is the whole thing `file.reveal` was supposed to do for them.
    set({
      count,
      chunks: new Map(),
      revealTo: index,
      selected: path,
      selectedIndex: index,
      // Exactly one row. A reveal is an answer to "where is this file", and leaving a
      // multi-row selection standing around the row it just jumped to would make the next
      // Delete act on rows from wherever the user was before.
      selection: collapseTo(path),
    })
    return true
  },

  async revealGroup(id) {
    const { project } = get()
    if (project === null) return false
    const path = groupPath(id)

    // Reveal first, expand second — the same order `beginDraft` uses and for the same reason:
    // expanding changes the row numbers *below* the header and leaves the header's own alone,
    // so the index from the reveal survives the expand and is not re-read.
    //
    // `fs_reveal` answers `null` when this project has no such group, which is every project
    // with no Cargo or Go manifest under a root. That is the honest "nothing to show" and the
    // caller says so.
    const at = await tree('fs_reveal', () => fsApi.reveal(project, path), null)
    if (at === null || at < 0 || get().project !== project) return false

    // The expand is what starts the resolution, and its answer is the new count — the group is
    // about to grow a `Resolving…` row, so the scroller has to be sized for it before the
    // virtualizer is told where to scroll.
    const count = await tree('fs_expand', () => fsApi.expand(project, path), get().count)
    if (get().project !== project) return false

    resetCache()
    set({
      count,
      chunks: new Map(),
      revealTo: at,
      selected: path,
      selectedIndex: at,
      selection: collapseTo(path),
    })
    return true
  },

  clearReveal() {
    set({ revealTo: null })
  },

  async refresh() {
    const { project } = get()
    if (project === null) return

    // The generation still moves — a `fs_tree_rows` issued before the burst describes the
    // flattening the burst just changed — but the *cache* does not: see the module header.
    // `inFlight` is cleared with it so those chunks can be asked for again; the loads
    // themselves bail on the generation check when they land.
    generation += 1
    const mine = generation
    inFlight = new Set()

    const count = await tree('fs_tree_count', () => fsApi.treeCount(project), get().count)
    if (generation !== mine || get().project !== project) return

    /*
     * Self-heal the writable set, and *only* when it is empty.
     *
     * `attach` asks once, and once is right — the answer moves when a project's roots move and
     * at no other time, and this runs on every watcher burst. But `attach` can lose: a project
     * whose index does not exist yet answers `NoIndex`, `tree()` falls back to `[]`, and nothing
     * would ever ask again. The tree would then work perfectly and grey *Rename…* on every
     * scratch for the life of the window, which is a dead control with no error anywhere — the
     * shape this whole milestone is about. The guard keeps the ordinary burst at zero calls.
     */
    if (get().writable.length === 0) {
      const writable = await tree('fs_writable_roots', () => fsApi.writableRoots(project), [])
      if (generation !== mine || get().project !== project) return
      if (writable.length > 0) set({ writable })
    }

    /*
     * Re-ask the reveal set when the composed row count moved, and the gate is **not** the one
     * above — the two lists become complete at different moments and for different reasons.
     *
     * `writable` is complete the moment it is non-empty, so "is it empty" is the right guard
     * there. This one is legitimately non-empty and still incomplete: an unresolved *External
     * Libraries* contributes nothing, so a cold project answers with exactly its roots, and
     * `fs_reveal_roots` deliberately does not resolve one (a `cargo metadata` inside a status
     * bar's render is not a trade worth making). Guarding on emptiness would therefore leave
     * every library crumb inert for the life of the window — the dead-affordance failure one
     * layer down from the one the crumbs exist to fix.
     *
     * A changed count is the exact signal, not a proxy, and the argument is worth writing down
     * because it is what keeps the ordinary burst at zero calls. **Every path that grows this
     * list fulfils a group while that group is expanded**: an expand asks for the resolution
     * (`Expanded::Resolve`), `reveal_path`'s `resolve_now` runs only for a path it is about to
     * reveal *into* the group (which opens it), and the stale-stamp path answers `Resolve` only
     * for a group that was expanded. An expanded group's children are visible rows, so the
     * composed count moves with them. A burst from an ordinary file *write* does not move the
     * count and costs nothing here; a file being created or deleted does, which is one cheap
     * call — two locks and a `Vec` clone — on a gesture the user made.
     *
     * The answer is compared before it is written: `revealable` is a prop of the status bar, and
     * a fresh array identity would re-render it for nothing.
     */
    /*
     * ...or while we still have no roots at all, which is the case the count cannot see.
     *
     * The row count only moves for an EXPANDED group. The picker's library scope resolves
     * *External Libraries* through its own path — the toggle forks `cargo metadata` and fills the
     * group — and it does that whether or not the group is open in the tree. Resolved while
     * collapsed, the composed count is unchanged, this call never runs, and `revealable` stays
     * empty: every crumb of a library path is drawn inert, including the package directory,
     * although Rust now knows exactly where the package roots are.
     *
     * Asking again while the list is empty is the cheap half of the question — `fs_reveal_roots`
     * deliberately does not resolve anything itself (see its note above), so an empty answer
     * costs two locks and a clone, and it stops costing anything the moment it succeeds once.
     *
     * The honest residual: this heals on the NEXT refresh rather than at the moment the picker
     * finishes. A user who resolves the scope and immediately looks at the status bar without
     * touching anything still sees inert crumbs until something moves the tree. Closing that
     * needs the picker's walk to nudge this store, which is a channel neither module has, and
     * inventing one for a one-frame staleness is the more expensive mistake.
     */
    if (count !== get().count || get().revealable.length === 0) {
      const revealable = await tree('fs_reveal_roots', () => fsApi.revealRoots(project), [])
      if (generation !== mine || get().project !== project) return
      if (revealable.length > 0 && !sameList(get().revealable, revealable)) set({ revealable })
    }

    const wanted = [...visible].filter((chunk) => chunk * CHUNK_ROWS < count)
    const fetched = await Promise.all(wanted.map((chunk) => readChunk(project, chunk, count)))
    if (generation !== mine || get().project !== project) return

    const before = get()
    const fresh = new Map<number, TreeRow[]>()
    let moved = count !== before.count
    wanted.forEach((chunk, i) => {
      const rows = fetched[i] ?? []
      fresh.set(chunk, rows)
      // No early exit on the first difference: every visible chunk has to end up in `fresh`
      // whatever the answer, or the single write below would blank the ones it skipped.
      moved ||= !sameRows(before.chunks.get(chunk), rows)
    })

    if (!moved) {
      // Nothing the tree is showing moved, so the tree must not re-render — no `set`, not even
      // one that writes back an equal value, because `chunks` is a Map and a fresh identity is
      // a re-render of every visible row. What is *not* on screen may still have moved, and
      // that is what the stale marks are for.
      for (const chunk of before.chunks.keys()) if (!visible.has(chunk)) stale.add(chunk)
      return
    }

    // One write, already carrying every visible row, so there is no frame in which the tree
    // has fewer rows than it had. The chunks outside the viewport are dropped rather than
    // carried over: the flattening moved, so their row numbers no longer mean anything, and
    // keeping them would put rows from two different trees on screen at once the moment the
    // user scrolled.
    stale = new Set()
    touched = [...wanted]
    set({ count, chunks: fresh })
  },

  async beginDraft(parent, directory, atTop) {
    const { project } = get()
    if (project === null) return false

    if (atTop) {
      // The hidden root of a single-root project. There is no row to anchor under, so the
      // draft is row 0 and the whole tree shifts down by one. Depth 0 for the same reason:
      // the top-level rows of a single-root project are the root's children at depth 0.
      const siblings = await readSiblings(project, 0, -1)
      if (get().project !== project) return false
      set({ draft: { parent, directory, index: 0, depth: 0, siblings }, revealTo: 0 })
      return true
    }

    const mine = generation
    const at = await tree('fs_reveal', () => fsApi.reveal(project, parent), null)
    if (generation !== mine || get().project !== project) return false
    // The parent has no row: gitignored, or deleted between the right-click and now. Rust
    // would refuse the create anyway; declining to open a box that cannot lead anywhere is
    // the earlier and quieter version of the same answer, and the caller reports it.
    if (at === null || at < 0) return false

    const count = await tree('fs_expand', () => fsApi.expand(project, parent), get().count)
    if (generation !== mine || get().project !== project) return false

    // Expanding re-flattened everything below the parent, so every cached chunk past it is
    // wrong. The two reads below therefore go through `fsApi` rather than the cache.
    resetCache()
    const parentRow = await tree(
      'fs_tree_rows',
      () => fsApi.treeRows(project, at, 1),
      [] as TreeRow[],
    )
    const depth = parentRow[0]?.depth ?? 0
    const siblings = await readSiblings(project, at + 1, depth)
    if (get().project !== project) return false

    set({
      count,
      chunks: new Map(),
      revealTo: at,
      draft: { parent, directory, index: at + 1, depth: depth + 1, siblings },
    })
    return true
  },

  cancelDraft() {
    if (get().draft === null) return
    set({ draft: null })
  },

  async commitDraft(name) {
    const { project, draft } = get()
    if (project === null || draft === null) return null

    // Not wrapped in `tree()`: a rejection here is the user's to see. `pendingCommand` would
    // swallow it into a fallback and latch `degraded`, which is right for a tree read that
    // can be answered with an empty window and exactly wrong for a write the user asked for.
    const created = await fsCreate.entry(project, draft.parent, name, draft.directory)

    // The editor goes before the refresh, not after: the row it was standing in for now
    // exists, and leaving both on screen for the length of a round trip draws the file twice.
    set({ draft: null })
    await get().refresh()
    if (get().project !== project) return null

    // `indexOf` searches the *resident* chunks, so a miss means one of two very different
    // things: the file genuinely has no row, or the cache does not currently hold the chunk
    // it is in. `refresh` above can leave the cache untouched — it bails on its generation
    // guard whenever anything else invalidated the tree while it was in flight, and the
    // watcher's own event for the file we just created is exactly such a thing. Answering
    // `null` on a cache miss therefore told the user "your ignore rules keep it out of the
    // tree" about an ordinary `main.rs`, which is both wrong and unfixable-sounding.
    //
    // So the cache miss is re-asked of Rust, which is the only place that knows. `fs_reveal`
    // answers `null` only when the path is not in the index at all — which is the claim the
    // message actually wants to make. One extra round trip, on the miss path only.
    const cached = get().indexOf(created)
    const at =
      cached ?? (await tree('fs_reveal', () => fsApi.reveal(project, created), null))
    if (at === null || at < 0 || get().project !== project) {
      // Created, but no row — the project's ignore rules hide it. `.gitignore`d dot-files are
      // the ordinary case. Reported rather than papered over: a selection pointing at a path
      // with no row is a highlight nobody can see.
      return null
    }
    // Selected, **not** opened. Creating a file is not opening it — the same rule a single
    // click follows. `reveal` would scroll as well, and `refresh` has already kept the
    // viewport where the draft was, which is where this row is.
    set({ selected: created, selectedIndex: at, selection: collapseTo(created) })
    return created
  },
}))

/**
 * Carry out what `treeSelection` decided, resolving a Shift-band if that is what it asked for.
 *
 * Outside the store rather than a method on it because both entry points ([`pressRow`] and
 * [`keyToRow`]) run the identical second half, and the only thing they disagree about is which
 * rule produced the plan.
 *
 * Resolves to the sentence the panel should show, or `null`.
 */
/**
 * Do these two selections name the same rows from the same anchor?
 *
 * Identity first, which is the case that matters: a deferred press hands back the very object it
 * was given. The walk after it is bounded by `RANGE_ROWS` and only ever runs when the two are the
 * same size, so the ordinary "collapse 10 000 rows to one" answers `false` on the size check.
 */
function same(a: TreeSelection, b: TreeSelection): boolean {
  if (a === b) return true
  if (a.anchor !== b.anchor || a.paths.size !== b.paths.size) return false
  for (const path of a.paths) if (!b.paths.has(path)) return false
  return true
}

async function runPlan(
  plan: SelectPlan,
  to: string,
  index: number,
  set: (partial: Partial<FileTreeStore>) => void,
  get: () => FileTreeStore,
): Promise<string | null> {
  if (plan.kind === 'set') {
    // The same guard `select` carries, and it moved here when every mouse press started coming
    // through `pressRow`: a click on the row the cursor is already on is the common case — it is
    // the first half of every double-click — and an unconditional write would hand every
    // subscriber a new `Set` holding the same one path, re-rendering every visible row to draw
    // exactly what it is drawing. The deferred press relies on it too: its plan is the *current*
    // selection by identity, so this is the line that makes "nothing yet" cost nothing.
    const state = get()
    if (state.selected === to && state.selectedIndex === index && same(state.selection, plan.next)) {
      return null
    }
    set({ selection: plan.next, selected: to, selectedIndex: index })
    return null
  }

  const from = get().indexOf(plan.from)
  if (from === null) {
    // The anchor has no resident row, so the band has no top. `applyRange` degrades to the
    // plain rule; see its comment for the two ordinary ways this happens.
    set({ selection: applyRange(get().selection, null, to), selected: to, selectedIndex: index })
    return null
  }

  const lo = Math.min(from, index)
  const hi = Math.max(from, index)
  const refusal = rangeRefusal(hi - lo + 1)
  // Refused *before* the cursor moves. A Shift-click that reports "too many rows" and also
  // silently relocates the anchor would leave the next Shift-click extending from somewhere the
  // user never put it.
  if (refusal !== null) return refusal

  // The resident case is answered in one write, without ever awaiting. It is also the common
  // one — Shift+↓ extends by a row that is on screen by definition — and the write has to be
  // single: splitting it would move the cursor a microtask before the band caught up, so a held
  // arrow key would draw the leading rule ahead of the ground it sits on.
  const resident = residentBand(get().chunks, lo, hi)
  if (resident !== null) {
    set({
      selection: applyRange(get().selection, resident, to),
      selected: to,
      selectedIndex: index,
    })
    return null
  }

  // Only when the band has to be fetched: the cursor first, the rows when they land. An arrow
  // key whose cursor waits on IPC is an arrow key that feels broken, and nothing else in this
  // handler depends on the paths having arrived.
  const before = get().selection
  set({ selected: to, selectedIndex: index })
  const paths = await fetchBand(lo, hi, get)
  // Anything the user did to the selection while the rows were in flight outranks them.
  if (paths === null || get().selection !== before) return null
  set({ selection: applyRange(before, paths, to) })
  return null
}

/**
 * The paths of rows `[lo, hi]`, from the cache when it holds them and from Rust when it does
 * not. `null` when the tree was re-flattened under the fetch.
 *
 * `runPlan` deliberately does **not** call this — it needs the resident case to be a synchronous
 * write, so it asks [`residentBand`] itself and falls through to [`fetchBand`]. This is the
 * combined form for Ctrl+A, which has no cursor to keep in step and where the whole tree is
 * resident only on a small repository anyway.
 */
async function bandPaths(
  lo: number,
  hi: number,
  get: () => FileTreeStore,
): Promise<string[] | null> {
  return residentBand(get().chunks, lo, hi) ?? (await fetchBand(lo, hi, get))
}

/**
 * Ask Rust for the paths of rows `[lo, hi]`. `null` when the tree moved under the request.
 *
 * Separate from the cache lookup because the caller that matters branches on which one it got:
 * a band already in the cache is answered without a microtask, and one that is not costs a
 * round trip. `RANGE_ROWS` (10 000) is under `CHUNK_CAP * CHUNK_ROWS` (12 800), so even a band
 * at the cap *can* be answered entirely from the cache.
 */
async function fetchBand(
  lo: number,
  hi: number,
  get: () => FileTreeStore,
): Promise<string[] | null> {
  const project = get().project
  if (project === null) return null
  const mine = generation
  const rows = await tree(
    'fs_tree_rows',
    () => fsApi.treeRows(project, lo, hi - lo + 1),
    [] as TreeRow[],
  )
  // The same guard every other read in this file carries: an expand, a collapse or a watcher
  // burst that landed while this was in flight has renumbered the rows these paths came from.
  if (generation !== mine || get().project !== project) return null
  // Deliberately whatever came back rather than the length that was asked for. A tree that
  // shrank under the request answers short, and selecting the rows that do exist is a better
  // answer than refusing the gesture.
  return rows.map((row) => row.path)
}

/** Paths for `[lo, hi]` out of the resident chunks, or `null` if a single row is missing. */
function residentBand(chunks: Map<number, TreeRow[]>, lo: number, hi: number): string[] | null {
  const paths: string[] = []
  for (let i = lo; i <= hi; i++) {
    const row = chunks.get(chunkOf(i))?.[i % CHUNK_ROWS]
    if (row === undefined) return null
    paths.push(row.path)
  }
  return paths
}

/**
 * How many rows one sibling scan reads.
 *
 * A directory with more children than this gets a partial list, so the "already exists" check
 * can miss and the user finds out from Rust's refusal instead — later, but never wrong.
 * Reading the whole of a `node_modules` to warn about a name nobody is typing is the trade
 * going the other way.
 */
const SIBLING_SCAN = 512

/**
 * The names of the rows at `depth + 1` starting at `from`, i.e. one directory's children.
 *
 * Read from Rust rather than from the row cache, because every caller has just re-flattened
 * the tree and the cache is empty by construction. The scan stops at the first row that is
 * *not* deeper than the parent, which is the parent's next sibling — everything between is
 * its subtree, and only the shallowest level of that subtree is a sibling of the new entry.
 */
async function readSiblings(project: ProjectId, from: number, depth: number): Promise<string[]> {
  const rows = await tree(
    'fs_tree_rows',
    () => fsApi.treeRows(project, from, SIBLING_SCAN),
    [] as TreeRow[],
  )
  const names: string[] = []
  for (const row of rows) {
    if (row.depth <= depth) break
    if (row.depth === depth + 1) names.push(row.name)
  }
  return names
}

/** Fetch one chunk and merge it in, evicting whatever the cap no longer allows. */
async function loadChunk(
  project: ProjectId,
  chunk: number,
  visible: ReadonlySet<number>,
  set: (partial: Partial<FileTreeStore>) => void,
  get: () => FileTreeStore,
): Promise<void> {
  const mine = generation
  // A chunk that starts past the end is not a request. `ensure` clamps to `count`, so this is
  // only reachable if the tree shrank under a queued chunk — but caching the empty answer
  // would leave those rows blank until something else invalidated them.
  if (chunk * CHUNK_ROWS >= get().count) {
    inFlight.delete(chunk)
    return
  }
  const rows = await readChunk(project, chunk, get().count)

  inFlight.delete(chunk)
  // The tree was re-flattened while this was in flight. These rows describe a shape that no
  // longer exists; writing them would show files at the wrong indentation under the wrong
  // parent, which looks like corruption rather than staleness.
  if (generation !== mine || get().project !== project) return
  stale.delete(chunk)

  // A revalidation that found nothing. Returning before the `set` is the whole point: the
  // rows on screen are already these rows, and replacing the Map would re-render the tree to
  // draw exactly what it is drawing.
  const previous = get().chunks.get(chunk)
  if (sameRows(previous, rows) && get().degraded === isDegraded('fs_tree_rows')) return

  const next = new Map(get().chunks)
  next.set(chunk, rows)
  for (const victim of chunksToEvict(touched, visible, CHUNK_CAP)) {
    next.delete(victim)
    const at = touched.indexOf(victim)
    if (at >= 0) touched.splice(at, 1)
  }
  set({ chunks: next, degraded: isDegraded('fs_tree_rows') })
}
