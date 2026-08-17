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
  /**
   * What `fs_reveal_roots` currently answers. Mutable, because *External Libraries* resolving
   * mid-session is the whole reason the store re-asks at all. (M16)
   */
  revealRoots: string[]
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
    // `null` for every walked row; only a synthetic group row carries one. Present because
    // `TreeRow` requires it — which is exactly the compile error `ROW_FIELDS` is there to force.
    detail: null,
  }
}

const backend: Backend = { rows: [], calls: new Map(), revealRoots: ['/p'] }

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
      case 'fs_writable_roots':
        // M13. `attach` asks once, after the count, so a project's scratch drawer is known
        // before the context menu needs it. The drawer itself is deliberately absent from this
        // fixture: nothing here draws a menu, and the property under test is that a *refresh*
        // never asks this again — a `fs_writable_roots` per watcher burst would be an IPC round
        // trip per burst for an answer that only moves when a project's roots do.
        return Promise.resolve(['/p'])
      case 'fs_reveal_roots':
        // M16. The status bar's clickable path trail asks what the tree can hang a row from:
        // the roots, plus every package under *External Libraries* and every file in the
        // scratch drawer. Mutable in this fixture because the property under test is that the
        // store re-asks **exactly when the composed count moved** — a group resolving always
        // grows the count, and an ordinary rename never does, so a burst that renames a row
        // must not pay for this and a burst that inserts one must.
        return Promise.resolve([...backend.revealRoots])
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
 * What `FileTree` subscribes to that a **refresh** can move, and with which equality.
 *
 * Not every `useFileTree(...)` call in the component: `selected`, `selection` and `draft` are
 * subscribed there too and are deliberately absent here, because `refresh` never writes them.
 * This list exists to answer one question — did a watcher burst re-render the tree — so a
 * selector the burst cannot touch would only be a row of the digest that is always `false`.
 * A field that *starts* being written by `refresh` belongs here on the same commit.
 */
const SELECTORS: Array<{ name: string; read: () => unknown; equal: (a: unknown, b: unknown) => boolean }> = [
  { name: 'count', read: () => useFileTree.getState().count, equal: Object.is },
  { name: 'chunks', read: () => useFileTree.getState().chunks, equal: Object.is },
  { name: 'degraded', read: () => useFileTree.getState().degraded, equal: Object.is },
  { name: 'revealTo', read: () => useFileTree.getState().revealTo, equal: Object.is },
  // Written by `attach` and by nothing else. Listed anyway, and that is the point of listing
  // it: the day a refresh starts writing it, this row turns true and the digest reports a
  // re-render per watcher burst rather than letting one arrive silently.
  { name: 'writable', read: () => useFileTree.getState().writable, equal: Object.is },
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
  // What the status bar's crumbs have to go on before any burst has happened, which is most of
  // a session: `attach` asks once, and a store that only self-healed would leave every crumb
  // inert until somebody created a file.
  const revealAtAttach = [...useFileTree.getState().revealable]
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
  // …and, in the same burst, *External Libraries* finishing its resolution. That is not two
  // unrelated events: fulfilling a group happens while the group is expanded, so its packages
  // become visible rows and the composed count moves with them. This is the burst the status
  // bar's crumbs have to learn from.
  backend.revealRoots = ['/p', '/dep/serde-1.0.229']
  await useFileTree.getState().refresh()
  await settle(5)
  const changed = view.take()
  const changedCalls = callsSince(mark)
  const rowFive = useFileTree.getState().rowAt(5)?.name ?? null
  const revealedAfterChange = [...useFileTree.getState().revealable]
  // Read here rather than at the end: a later burst appends a row of its own, and a count read
  // out of the final state would be describing that instead of this insertion.
  const changedCount = useFileTree.getState().count

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
  // A third answer nobody should ever see. A rename moves no row *count*, so this burst must not
  // ask — which is what makes the gate above a gate rather than "ask on every burst" with a
  // comment claiming otherwise.
  backend.revealRoots = ['/p', '/dep/serde-1.0.229', '/dep/syn-2.0.87']
  await useFileTree.getState().refresh()
  await settle(5)
  const offscreen = view.take()
  const offscreenCalls = callsSince(mark)
  const revealedAfterOffscreen = [...useFileTree.getState().revealable]

  // Scrolling back to it revalidates. The old row is still readable the whole time — the tree
  // never blanks it — and the new one is there once the chunk lands.
  const staleName = useFileTree.getState().rowAt(250)?.name ?? null
  view.look(210, 260)
  const duringRevalidate = useFileTree.getState().rowAt(250)?.name ?? null
  await settle(5)
  const revalidatedName = useFileTree.getState().rowAt(250)?.name ?? null

  // --- a burst that moves the count while the reveal set stays exactly the same --------------
  //
  // The ordinary case for the rest of a session: a file is created, so the count moves and the
  // list is re-asked, and the answer is the one the store already holds. It must be dropped
  // rather than written back — `revealable` is a *prop of the status bar*, and a fresh array of
  // equal strings re-renders it and re-runs its crumb classification for nothing.
  //
  // Appended rather than inserted, so no row above shifts and this cannot disturb the four
  // assertions above it.
  backend.revealRoots = ['/p', '/dep/serde-1.0.229']
  const revealBefore = useFileTree.getState().revealable
  backend.rows.push(row('/p/another.rs'))
  await useFileTree.getState().refresh()
  await settle(5)
  view.take()
  const revealIdentityHeld = useFileTree.getState().revealable === revealBefore

  // --- and a project switch drops it, synchronously ------------------------------------------
  //
  // Read without awaiting, deliberately: `attach` clears its state before its first `await`, and
  // the window this is about is exactly that — the frames between switching project and the new
  // answer landing. A reveal set left standing there classifies the new project's trail against
  // the old project's roots, and the click reveals a path that is not in this tree.
  void useFileTree.getState().attach('p2')
  const revealClearedOnSwitch = [...useFileTree.getState().revealable]

  view.stop()
  console.log(
    JSON.stringify({
      primed: { renders: primed.renders, rows: primed.rows },
      quiet: { renders: quiet.renders, drawn: quiet.drawn, rows: quiet.rows, calls: quietCalls },
      changed: {
        renders: changed.renders,
        drawn: changed.drawn,
        rows: changed.rows,
        count: changedCount,
        rowFive,
        calls: changedCalls,
        revealable: revealedAfterChange,
      },
      offscreen: {
        renders: offscreen.renders,
        drawn: offscreen.drawn,
        calls: offscreenCalls,
        revealable: revealedAfterOffscreen,
        staleName,
        duringRevalidate,
        revalidatedName,
      },
      revealAtAttach,
      revealIdentityHeld,
      revealClearedOnSwitch,
    }),
  )
}

void main()
