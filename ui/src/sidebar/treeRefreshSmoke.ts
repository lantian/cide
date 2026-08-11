/**
 * The file tree under a watcher burst, driven headlessly. Read by `scripts/check-tree-flicker.mjs`.
 *
 * The user's complaint was "the project tree is flickering — it seems it's updating, but it
 * should not be flickering". Two things have to be true for that to stop, and neither can be
 * seen in a screenshot:
 *
 *  1. a burst that moved no row the tree is showing must cost **zero re-renders**, and
 *  2. a burst that did move rows must land in **one** update that already carries the new
 *     rows — no intermediate state in which the tree has fewer rows than it had.
 *
 * Both are statements about a sequence of store updates over time, so this drives the real
 * `treeStore` and the real `gitStatusStore` against a fake `invoke` and records what a
 * subscriber would have seen. There is no DOM here: `renders` is counted the way React
 * counts them under `useSyncExternalStore` — a selector whose snapshot is identical to the
 * last one does not re-render the component — with one entry per selector `FileTree`
 * actually subscribes to. That is a model, and it is pinned to the component by
 * `SELECTORS` below; if `FileTree` grows a subscription, this has to grow one too.
 */
import { useFileTree } from './treeStore'
import { useGitStatus } from './gitStatusStore'
import type { TreeRow } from '@/ipc/client'

interface Backend {
  /** The flattened tree, as `fs_tree_rows` would window it. */
  rows: TreeRow[]
  /** Command name → how many times it was invoked. */
  calls: Map<string, number>
}

function row(path: string, depth = 0): TreeRow {
  return {
    path,
    name: path.slice(path.lastIndexOf('/') + 1),
    depth,
    kind: 'file',
    expanded: false,
    hasChildren: false,
    symlink: false,
    root: 0,
  }
}

const backend: Backend = { rows: [], calls: new Map() }

function install(): void {
  const invoke = (cmd: string, args: Record<string, unknown>): Promise<unknown> => {
    backend.calls.set(cmd, (backend.calls.get(cmd) ?? 0) + 1)
    switch (cmd) {
      case 'fs_tree_count':
        return Promise.resolve(backend.rows.length)
      case 'fs_tree_rows': {
        const offset = args['offset'] as number
        const len = args['len'] as number
        return Promise.resolve(backend.rows.slice(offset, offset + len))
      }
      case 'git_tree_status':
        // Deliberately a *fresh object with equal contents* on every call, which is what
        // `gitStatusStore` installs after every refresh. It is the case `FileTree`'s
        // `useShallow` exists for.
        return Promise.resolve({ statuses: { '/p/a-0': 'modified' }, truncated: false })
      default:
        return Promise.reject(new Error(`unexpected command ${cmd}`))
    }
  }
  ;(globalThis as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = { invoke }
}

/** Let every pending microtask and the git store's 120 ms coalescing window run out. */
function settle(ms = 0): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

/**
 * What `FileTree` subscribes to, and with which equality. One entry per `useFileTree(...)` /
 * `useGitStatus(...)` call in the component.
 */
const SELECTORS: Array<{ name: string; read: () => unknown; equal: (a: unknown, b: unknown) => boolean }> = [
  { name: 'count', read: () => useFileTree.getState().count, equal: Object.is },
  { name: 'chunks', read: () => useFileTree.getState().chunks, equal: Object.is },
  { name: 'degraded', read: () => useFileTree.getState().degraded, equal: Object.is },
  { name: 'revealTo', read: () => useFileTree.getState().revealTo, equal: Object.is },
  {
    name: 'statuses',
    read: () => useGitStatus.getState().status.statuses,
    equal: (a, b) => shallowEqual(a as Record<string, unknown>, b as Record<string, unknown>),
  },
]

function shallowEqual(a: Record<string, unknown>, b: Record<string, unknown>): boolean {
  if (Object.is(a, b)) return true
  const ka = Object.keys(a)
  const kb = Object.keys(b)
  return ka.length === kb.length && ka.every((k) => Object.is(a[k], b[k]))
}

/**
 * A subscriber that behaves like the mounted component: it re-renders when a selector's
 * snapshot changes, and records what it would have drawn each time it did.
 */
class View {
  renders = 0
  /** Rows resolved per render, for the viewport it is looking at. */
  drawn: number[] = []
  private snapshots = SELECTORS.map((s) => s.read())
  private from = 0
  private to = 0
  private unsubscribe: Array<() => void> = []

  constructor() {
    const react = () => {
      let changed = false
      SELECTORS.forEach((selector, i) => {
        const next = selector.read()
        if (!selector.equal(this.snapshots[i], next)) changed = true
        this.snapshots[i] = next
      })
      if (!changed) return
      this.renders += 1
      this.drawn.push(this.rows())
    }
    this.unsubscribe.push(useFileTree.subscribe(react))
    this.unsubscribe.push(useGitStatus.subscribe(react))
  }

  /** Point the viewport at `[from, to)` and run the effect the component runs after a render. */
  look(from: number, to: number): void {
    this.from = from
    this.to = to
    useFileTree.getState().ensure(from, to)
  }

  /** How many of the viewport's indices resolve to a row rather than a placeholder. */
  rows(): number {
    const state = useFileTree.getState()
    let n = 0
    for (let i = this.from; i < Math.min(this.to, state.count); i++) {
      if (state.rowAt(i) !== undefined) n += 1
    }
    return n
  }

  /** Renders and rows drawn since the last call, then reset. */
  take(): { renders: number; drawn: number[]; rows: number } {
    const out = { renders: this.renders, drawn: this.drawn, rows: this.rows() }
    this.renders = 0
    this.drawn = []
    return out
  }

  stop(): void {
    for (const fn of this.unsubscribe) fn()
  }
}

function callsSince(before: Map<string, number>): Record<string, number> {
  const out: Record<string, number> = {}
  for (const [cmd, n] of backend.calls) {
    const delta = n - (before.get(cmd) ?? 0)
    if (delta > 0) out[cmd] = delta
  }
  return out
}

async function main(): Promise<void> {
  install()
  // 500 rows: more than the 200-row chunk, so a viewport at the top and one at row 210 are
  // different chunks and "off screen" is a real state rather than a hypothetical.
  backend.rows = Array.from({ length: 500 }, (_, i) => row(`/p/a-${i}`))

  await useFileTree.getState().attach('p1')
  await useGitStatus.getState().attach('p1')
  const view = new View()
  view.look(0, 30)
  await settle(5)
  view.look(0, 30)
  await settle(5)
  const primed = view.take()

  // --- a burst that changes nothing ---------------------------------------------------------
  //
  // A `git status` in a terminal pane rewrites `.git/index`, which is watched on purpose and
  // which `Index::apply` skips by name. This is that burst.
  let mark = new Map(backend.calls)
  await useFileTree.getState().refresh()
  useGitStatus.getState().schedule()
  await settle(200)
  const quiet = view.take()
  const quietCalls = callsSince(mark)

  // --- a burst that inserts a row above the viewport ------------------------------------------
  //
  // The naive fix misses this one: dropping the cache and re-fetching draws one frame with no
  // rows at all before the new ones land.
  mark = new Map(backend.calls)
  backend.rows.splice(5, 0, row('/p/new.rs'))
  await useFileTree.getState().refresh()
  await settle(5)
  const changed = view.take()
  const changedCalls = callsSince(mark)
  const rowFive = useFileTree.getState().rowAt(5)?.name ?? null

  // --- a burst that only touches rows the user cannot see -------------------------------------
  //
  // Chunk 1 is resident but off screen. Renaming a row inside it moves no visible row and must
  // still not leave the tree showing a name that is gone.
  view.look(210, 240)
  await settle(5)
  view.look(0, 30)
  await settle(5)
  view.take()
  mark = new Map(backend.calls)
  backend.rows[250] = row('/p/renamed.rs')
  await useFileTree.getState().refresh()
  await settle(5)
  const offscreen = view.take()
  const offscreenCalls = callsSince(mark)

  // Scrolling back to it revalidates. The old row is still readable the whole time — the tree
  // never blanks it — and the new one is there once the chunk lands.
  const staleName = useFileTree.getState().rowAt(250)?.name ?? null
  view.look(210, 260)
  const duringRevalidate = useFileTree.getState().rowAt(250)?.name ?? null
  await settle(5)
  const revalidatedName = useFileTree.getState().rowAt(250)?.name ?? null

  view.stop()
  console.log(
    JSON.stringify({
      primed: { renders: primed.renders, rows: primed.rows },
      quiet: { renders: quiet.renders, drawn: quiet.drawn, rows: quiet.rows, calls: quietCalls },
      changed: {
        renders: changed.renders,
        drawn: changed.drawn,
        rows: changed.rows,
        count: useFileTree.getState().count,
        rowFive,
        calls: changedCalls,
      },
      offscreen: {
        renders: offscreen.renders,
        drawn: offscreen.drawn,
        calls: offscreenCalls,
        staleName,
        duringRevalidate,
        revalidatedName,
      },
    }),
  )
}

void main()
