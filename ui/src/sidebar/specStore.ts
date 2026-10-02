/** Revisioned board snapshots; requests are shared while pending. */
import { create } from 'zustand'
import { spec as specApi, type ProjectId, type SpecSnapshot as WireBoard } from '@/ipc/client'
import { adaptBoard } from './OpenSpecPanel/adapt'
import { invalidateSpecData } from './OpenSpecPanel/specData'
import { BOARD_UNKNOWN, newerBoard, type Board } from './OpenSpecPanel/model'

interface SpecStore {
  project: ProjectId | null
  /** What `openspec/` last said. **Never null** — see `BOARD_UNKNOWN`. */
  board: Board
  revision: number
  /** True while a gesture is in flight, so a button cannot be pressed twice. */
  busy: boolean
  attach: (project: ProjectId | null) => void
  refresh: (force?: boolean) => Promise<void>
  adopt: (project: ProjectId, board: WireBoard) => void
  setUp: () => Promise<void>
}

const pending = new Map<ProjectId, { promise: Promise<void>; force: boolean }>()
const forceQueued = new Map<ProjectId, Promise<void>>()

export const useSpec = create<SpecStore>((set, get) => ({
  project: null,
  board: BOARD_UNKNOWN,
  revision: 0,
  busy: false,

  attach: (project) => {
    if (get().project === project) return
    // Reset to `unknown` and not to `absent`: until the read answers, cide has not looked, and
    // the absent screen offers to write a directory into the repository.
    set({ project, board: BOARD_UNKNOWN, revision: 0 })
    if (project !== null) void get().refresh()
  },

  refresh: (force = false) => {
    const project = get().project
    if (project === null) return Promise.resolve()
    const existing = pending.get(project)
    if (existing) {
      if (!force || existing.force) return existing.promise
      const queued = forceQueued.get(project)
      if (queued) return queued
      const retry = existing.promise.then(() => get().project === project ? get().refresh(true) : undefined)
        .finally(() => { forceQueued.delete(project) })
      forceQueued.set(project, retry)
      return retry
    }
    const before = get().revision
    const request = specApi.board(project, force).then((wire) => {
      if (get().project !== project || wire === null) return
      if (force) invalidateSpecData(project, wire, { full: true, paths: [] })
      get().adopt(project, wire)
    }).catch((error: unknown) => {
      if (get().project === project && get().revision === before) set({ board: { kind: 'unusable', reason: String(error) } })
    }).finally(() => { if (pending.get(project)?.promise === request) pending.delete(project) })
    pending.set(project, { promise: request, force })
    return request
  },

  adopt: (project, snapshot) => {
    if (get().project !== project || snapshot.revision < get().revision) return
    set((state) => {
      const next = newerBoard(state.board, adaptBoard(snapshot.board))
      // Keep identity for unchanged summaries; content invalidation is a separate signal.
      const board = JSON.stringify(next) === JSON.stringify(state.board) ? state.board : next
      return { board, revision: snapshot.revision }
    })
  },

  setUp: async () => {
    const project = get().project
    if (project === null || get().busy) return
    set({ busy: true })
    try {
      const wire = await specApi.init(project)
      if (get().project !== project) return
      get().adopt(project, wire)
    } finally {
      set({ busy: false })
    }
  },
}))
