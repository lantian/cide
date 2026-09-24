/**
 * The project's diagnostics, and what keeps them fresh.
 *
 * Modelled on `gitStatusStore` line for line, because it answers the same shape of question: a
 * per-project snapshot that several surfaces read, refreshed on events rather than on a timer.
 *
 * # Why the raw snapshot lives here and the filtered one does not
 *
 * This store holds what Rust said. The **filter is applied once, at the point of use**, by
 * `applyFilters` in `App.tsx` — and the result is handed to the panel, the status bar and the
 * rail badge together. Caching a filtered copy here would mean invalidating it whenever the
 * settings changed, which is a second source of truth about the same list; caching *both* would
 * be two. `model.ts:13-16` states the underlying rule: the bar and the panel are two renderings
 * of one snapshot precisely so they cannot disagree.
 *
 * # Why a throttle and not a debounce
 *
 * The same argument `gitStatusStore` makes: a restarting debounce can be starved indefinitely by
 * a steady stream of triggers, and `cargo watch` over a large tree is exactly that. Rust already
 * coalesces the *emit* at 250 ms with a 1 s ceiling, so this window only exists to collapse the
 * handful that still arrive together — a save that publishes for three files, say.
 */
import { create } from 'zustand'
import { diagnostics as diagnosticsApi, pendingCommand, type ProjectId } from '@/ipc/client'
import { adaptSnapshot } from './ProblemsPanel/adapt'
import { NO_SOURCE, type DiagnosticsSnapshot } from './ProblemsPanel/model'
import { shareEqual } from '@/store/shareEqual'

const COALESCE_MS = 120

interface DiagnosticsStore {
  project: ProjectId | null
  /** What Rust last said. Never null — see `NO_SOURCE`. */
  snapshot: DiagnosticsSnapshot

  /** Point the store at a project, or at nothing. Clears first. */
  attach: (project: ProjectId | null) => Promise<void>
  /** Re-read now. */
  refresh: () => Promise<void>
  /** Coalesced refresh, for event handlers. */
  schedule: () => void
}

let timer: ReturnType<typeof setTimeout> | null = null
/**
 * Claimed *before* each call and re-checked after, so an answer for a project the user has since
 * left is dropped rather than painted. The same guard `gitStatusStore` uses, and the same reason:
 * `attach` is called from a render effect and can be re-entered before the previous call lands.
 */
let generation = 0

export const useDiagnostics = create<DiagnosticsStore>((set, get) => ({
  project: null,
  snapshot: NO_SOURCE,

  attach: async (project) => {
    generation += 1
    if (timer !== null) {
      clearTimeout(timer)
      timer = null
    }
    /*
     * Cleared to `NO_SOURCE`, not left standing. The old project's diagnostics over the new
     * project's files is a *wrong* answer; "nothing is analysing this" is a true one that lasts
     * one round trip. This is the same call the panel's whole design turns on.
     */
    set({ project, snapshot: NO_SOURCE })
    if (project === null) return
    await get().refresh()
  },

  refresh: async () => {
    const project = get().project
    if (project === null) return
    generation += 1
    const mine = generation

    /*
     * `pendingCommand`, so a build without the handler degrades to the v1 explainer rather than
     * to a rejected promise. React 19 unmounts the whole tree on an unhandled throw out of an
     * effect, so a bare `await` here is a blank window — the same reasoning `useGitPanel`'s
     * `guarded` wrapper is built on.
     */
    const wire = await pendingCommand(
      'diagnostics_get',
      () => diagnosticsApi.get(project),
      null,
    )
    // `null` is `pendingCommand`'s "this build has no such handler" — distinct from an
    // `unavailable` snapshot, which is a real answer from a real handler. Both render the same
    // explainer, and conflating them here would lose the distinction for anything that later
    // wants it.
    const snapshot: DiagnosticsSnapshot = wire === null ? NO_SOURCE : adaptSnapshot(wire)
    // Both guards: a newer call has superseded this one, or the user moved to another project
    // while it was in flight.
    if (mine !== generation || get().project !== project) return
    /*
     * Shared with the snapshot already held, and not set at all when nothing in it moved.
     *
     * A busy server publishes continuously, every publish ends in this fetch, and most of them
     * change nothing a reader draws — a re-check that found the same problems. `App` reads this
     * snapshot for the rail's counts and the status bar, so each `set` re-rendered the whole
     * shell; the Problems panel's list re-rendered every row. Unchanged items now keep their
     * objects, and an unchanged snapshot is no update.
     */
    const held = get().snapshot
    const shared = shareEqual(held, snapshot)
    if (shared === held) return
    set({ snapshot: shared })
  },

  schedule: () => {
    // Not restarted by triggers that arrive while it is armed — see the header.
    if (timer !== null) return
    timer = setTimeout(() => {
      timer = null
      void get().refresh()
    }, COALESCE_MS)
  },
}))
