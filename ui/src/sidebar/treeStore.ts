/**
 * The file tree's windowed row cache.
 *
 * Rust owns the tree; this owns a bounded view of it. Three rules the M8 criteria turn on:
 *
 * 1. Rows are fetched in fixed-size chunks (see `rowWindow.ts`), never in whatever range the
 *    virtualizer happens to be showing, so requests deduplicate.
 * 2. Resident chunks are capped, so the JS heap stays flat regardless of repository size —
 *    scrolling a 100k-file tree end to end must not retain every row it passed.
 * 3. Any structural change — expand, collapse, a watcher event — drops the whole cache. A
 *    partial invalidation would need to know which rows moved, and the flattened tree
 *    renumbers everything after the change; a cache that is subtly wrong about row 900 shows
 *    the user a file that is not there.
 *
 * Everything reaches Rust through `pendingCommand`, because none of `fs_tree_rows`,
 * `fs_tree_count`, `fs_expand`, `fs_collapse` or `fs_reveal` exists yet. A missing handler
 * yields an empty tree and one log line, never a rejected promise inside a render.
 */
import { create } from 'zustand'
import { fs as fsApi, isDegraded, pendingCommand, type ProjectId, type TreeRow } from '@/ipc/client'
import { CHUNK_CAP, CHUNK_ROWS, chunkOf, chunkRequest, chunksFor, chunksToEvict } from './rowWindow'

interface FileTreeStore {
  project: ProjectId | null
  /** Total rows in the flattened tree. The virtualizer's `count`. */
  count: number
  /** Resident rows, keyed by chunk index. */
  chunks: Map<number, TreeRow[]>
  /** True once any `fs_*` call has failed, i.e. the commands are not in this build. */
  degraded: boolean
  /**
   * A row index the tree should scroll to, set by `reveal` and cleared by the component
   * once it has scrolled. A number rather than a boolean flag so two reveals in a row are
   * two scrolls.
   */
  revealTo: number | null

  /** Point the tree at a project, or at nothing. Reads the row count. */
  attach: (project: ProjectId | null) => Promise<void>
  /** Ask for whatever chunks cover `[from, to)`. Cheap and idempotent; call from render. */
  ensure: (from: number, to: number) => void
  /** The row at a flattened index, or `undefined` while its chunk is in flight. */
  rowAt: (index: number) => TreeRow | undefined
  /** Expand or collapse a directory row. */
  toggle: (row: TreeRow) => Promise<void>
  /** Expand ancestors until `path` is visible, then scroll to it. */
  reveal: (path: string) => Promise<void>
  clearReveal: () => void
  /** Drop every cached row and re-read the count. The `cide://fs-changed` handler. */
  refresh: () => Promise<void>
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
 * Bumped on every invalidation. A response tagged with an older generation is dropped —
 * without this, a `fs_tree_rows` reply that was in flight when the user collapsed a folder
 * lands afterwards and writes rows from the *previous* flattening into the new one.
 */
let generation = 0

function resetCache(): void {
  inFlight = new Set()
  touched = []
  generation += 1
}

export const useFileTree = create<FileTreeStore>((set, get) => ({
  project: null,
  count: 0,
  chunks: new Map(),
  degraded: false,
  revealTo: null,

  async attach(project) {
    resetCache()
    set({ project, count: 0, chunks: new Map(), revealTo: null })
    if (project === null) return

    const mine = generation
    const count = await pendingCommand('fs_tree_count', () => fsApi.treeCount(project), 0)
    if (generation !== mine) return
    set({ count, degraded: isDegraded('fs_tree_count') })
  },

  ensure(from, to) {
    const { project, count, chunks } = get()
    if (project === null || count === 0) return

    const wanted = chunksFor(Math.max(0, from), Math.min(to, count), CHUNK_ROWS)
    for (const chunk of wanted) {
      // Touch first, so a chunk being re-visited moves to the back of the eviction queue
      // even when it is already resident and no request is made.
      const at = touched.indexOf(chunk)
      if (at >= 0) touched.splice(at, 1)
      touched.push(chunk)

      if (chunks.has(chunk) || inFlight.has(chunk)) continue
      inFlight.add(chunk)
      void loadChunk(project, chunk, new Set(wanted), set, get)
    }
  },

  rowAt(index) {
    const chunk = get().chunks.get(chunkOf(index))
    return chunk?.[index % CHUNK_ROWS]
  },

  async toggle(row) {
    const { project } = get()
    if (project === null || !row.isDir) return

    const call = row.expanded
      ? () => fsApi.collapse(project, row.path)
      : () => fsApi.expand(project, row.path)
    const name = row.expanded ? 'fs_collapse' : 'fs_expand'

    // The count comes back from the same call rather than a follow-up `fs_tree_count`: the
    // two would be separate round trips against a tree that another expand could change
    // between them, and the virtualizer would briefly size itself to a count that never
    // matched the rows.
    const count = await pendingCommand(name, call, get().count)
    resetCache()
    set({ count, chunks: new Map(), degraded: isDegraded(name) })
  },

  async reveal(path) {
    const { project } = get()
    if (project === null) return

    const index = await pendingCommand('fs_reveal', () => fsApi.reveal(project, path), -1)
    // The project can be swapped out from under a reveal — `attach` is what a project switch
    // runs — and writing this count and this index into the new project's tree would scroll it
    // to a row belonging to the old one.
    if (index < 0 || get().project !== project) return

    // Revealing expands ancestors, so the flattening moved; everything cached is stale.
    resetCache()
    const mine = generation
    const count = await pendingCommand('fs_tree_count', () => fsApi.treeCount(project), get().count)
    // The same guard `attach` and `refresh` carry, for the same reason: an expand or a watcher
    // refresh that landed while this count was in flight has already re-flattened the tree, so
    // both the count and the row index below describe a shape that is gone.
    if (generation !== mine) return
    set({ count, chunks: new Map(), revealTo: index })
  },

  clearReveal() {
    set({ revealTo: null })
  },

  async refresh() {
    const { project } = get()
    if (project === null) return
    resetCache()
    const mine = generation
    const count = await pendingCommand('fs_tree_count', () => fsApi.treeCount(project), get().count)
    if (generation !== mine) return
    set({ count, chunks: new Map() })
  },
}))

/** Fetch one chunk and merge it in, evicting whatever the cap no longer allows. */
async function loadChunk(
  project: ProjectId,
  chunk: number,
  visible: ReadonlySet<number>,
  set: (partial: Partial<FileTreeStore>) => void,
  get: () => FileTreeStore,
): Promise<void> {
  const mine = generation
  const { offset, len } = chunkRequest(chunk, get().count)
  if (len <= 0) {
    inFlight.delete(chunk)
    return
  }

  const rows = await pendingCommand(
    'fs_tree_rows',
    () => fsApi.treeRows(project, offset, len),
    [] as TreeRow[],
  )

  inFlight.delete(chunk)
  // The tree was re-flattened while this was in flight. These rows describe a shape that no
  // longer exists; writing them would show files at the wrong indentation under the wrong
  // parent, which looks like corruption rather than staleness.
  if (generation !== mine || get().project !== project) return

  const next = new Map(get().chunks)
  next.set(chunk, rows)
  for (const victim of chunksToEvict(touched, visible, CHUNK_CAP)) {
    next.delete(victim)
    const at = touched.indexOf(victim)
    if (at >= 0) touched.splice(at, 1)
  }
  set({ chunks: next, degraded: isDegraded('fs_tree_rows') })
}
