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
import { CHUNK_CAP, CHUNK_ROWS, chunkOf, chunkRequest, chunksFor, chunksToEvict } from './rowWindow'

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
  /** Select a row. `index` is a hint for the arrows; see `selectedIndex`. */
  select: (path: string, index: number) => void
  /** Expand or collapse a directory row. */
  toggle: (row: TreeRow) => Promise<void>
  /** Expand ancestors until `path` is visible, then scroll to it. */
  reveal: (path: string) => Promise<void>
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
}
const ROW_KEYS = Object.keys(ROW_FIELDS) as Array<keyof TreeRow>

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
  revealTo: null,
  selected: null,
  selectedIndex: 0,
  draft: null,

  async attach(project) {
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
      draft: null,
    })
    if (project === null) return

    const mine = generation
    const count = await tree('fs_tree_count', () => fsApi.treeCount(project), 0)
    if (generation !== mine) return
    set({ count, degraded: isDegraded('fs_tree_count') })
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
    const { selected, selectedIndex } = get()
    // Guarded because a click on the already-selected row is the common case (it is the first
    // half of every double-click), and an unconditional `set` would re-render every visible
    // row to draw exactly what it is drawing.
    if (selected === path && selectedIndex === index) return
    set({ selected: path, selectedIndex: index })
  },

  async toggle(row) {
    const { project } = get()
    if (project === null || row.kind !== 'dir') return

    const call = row.expanded
      ? () => fsApi.collapse(project, row.path)
      : () => fsApi.expand(project, row.path)
    const name = row.expanded ? 'fs_collapse' : 'fs_expand'

    // The count comes back from the same call rather than a follow-up `fs_tree_count`: the
    // two would be separate round trips against a tree that another expand could change
    // between them, and the virtualizer would briefly size itself to a count that never
    // matched the rows.
    const count = await tree(name, call, get().count)
    resetCache()
    set({ count, chunks: new Map(), degraded: isDegraded(name) })
  },

  async reveal(path) {
    const { project } = get()
    if (project === null) return

    // `fs_reveal` answers `null` for a path outside the index — a stale picker row, or a
    // file deleted between the pick and the reveal. Treated as "nothing to scroll to"
    // rather than coerced to a row number.
    const index = await tree('fs_reveal', () => fsApi.reveal(project, path), null)
    // The project can be swapped out from under a reveal — `attach` is what a project switch
    // runs — and writing this count and this index into the new project's tree would scroll it
    // to a row belonging to the old one.
    if (index === null || index < 0 || get().project !== project) return

    // Revealing expands ancestors, so the flattening moved; everything cached is stale.
    resetCache()
    const mine = generation
    const count = await tree('fs_tree_count', () => fsApi.treeCount(project), get().count)
    // The same guard `attach` and `refresh` carry, for the same reason: an expand or a watcher
    // refresh that landed while this count was in flight has already re-flattened the tree, so
    // both the count and the row index below describe a shape that is gone.
    if (generation !== mine) return
    // A reveal scrolls to a row the user was not looking at, so it also selects it: landing
    // on a screenful of rows with nothing marked leaves them to find the file again by eye,
    // which is the whole thing `file.reveal` was supposed to do for them.
    set({ count, chunks: new Map(), revealTo: index, selected: path, selectedIndex: index })
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

    const at = get().indexOf(created)
    if (at === null) {
      // Created, but no row — the project's ignore rules hide it. `.gitignore`d dot-files are
      // the ordinary case. Reported rather than papered over: a selection pointing at a path
      // with no row is a highlight nobody can see.
      return null
    }
    // Selected, **not** opened. Creating a file is not opening it — the same rule a single
    // click follows. `reveal` would scroll as well, and `refresh` has already kept the
    // viewport where the draft was, which is where this row is.
    set({ selected: created, selectedIndex: at })
    return created
  },
}))

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
