/**
 * The project's OpenSpec board, and what keeps it fresh. (M28)
 *
 * `tasksStore`'s shape, and its rules apply unchanged:
 *
 * **Every write path goes through `adopt`, and `adopt` goes through `newerBoard`.** The drop rule
 * is narrower here than the tracker's because `cide://spec-changed` carries no revision — it does
 * not need one, since `spec_state`'s coalescer runs exactly one read at a time per project — but
 * the one drop that matters is the same: a real board must never be replaced by "nobody has
 * asked". `newerBoard` returns the identical object when it drops, so a reader selecting
 * `s.board` sees the same reference and does not re-render, and nothing here may defeat that by
 * wrapping the result or by calling `set` unconditionally.
 *
 * **The subscription lives in `App.tsx`, not in the host.** `gitCountStore`'s reason: the rail's
 * badge has to stay live while the sidebar is shut or showing Files, and a listener registered
 * inside a panel goes stale the moment that panel unmounts.
 *
 * # Why a board read is not free, and what that changes
 *
 * Unlike a tracker read, which is one file, a board is **two subprocesses**. So the event's
 * payload is adopted directly and Rust is never asked anything in response to it — the same rule
 * `tasksStore` follows for the same reason, but here breaking it would cost a Node process per
 * keystroke rather than a file read. `refresh` exists for the two moments that have no payload:
 * attaching to a project, and the Retry button on a board cide could not read.
 */
import { create } from 'zustand'
import { spec as specApi, type ProjectId, type SpecBoard as WireBoard } from '@/ipc/client'
import { adaptBoard } from './OpenSpecPanel/adapt'
import { BOARD_UNKNOWN, newerBoard, type Board } from './OpenSpecPanel/model'

interface SpecStore {
  project: ProjectId | null
  /** What `openspec/` last said. **Never null** — see `BOARD_UNKNOWN`. */
  board: Board
  /** True while a gesture is in flight, so a button cannot be pressed twice. */
  busy: boolean
  attach: (project: ProjectId | null) => void
  refresh: () => Promise<void>
  adopt: (project: ProjectId, board: WireBoard) => void
  setUp: () => Promise<void>
}

export const useSpec = create<SpecStore>((set, get) => ({
  project: null,
  board: BOARD_UNKNOWN,
  busy: false,

  attach: (project) => {
    if (get().project === project) return
    // Reset to `unknown` and not to `absent`: until the read answers, cide has not looked, and
    // the absent screen offers to write a directory into the repository.
    set({ project, board: BOARD_UNKNOWN })
    if (project !== null) void get().refresh()
  },

  refresh: async () => {
    const project = get().project
    if (project === null) return
    const wire = await specApi.board(project)
    // Re-read the project *after* the await: a project switch during a slow board read would
    // otherwise paint another project's specs.
    if (get().project !== project || wire === null) return
    set((state) => {
      const next = newerBoard(state.board, adaptBoard(wire))
      return next === state.board ? state : { board: next }
    })
  },

  adopt: (project, board) => {
    if (get().project !== project) return
    set((state) => {
      const next = newerBoard(state.board, adaptBoard(board))
      return next === state.board ? state : { board: next }
    })
  },

  setUp: async () => {
    const project = get().project
    if (project === null || get().busy) return
    set({ busy: true })
    try {
      const wire = await specApi.init(project)
      if (get().project !== project) return
      set((state) => {
        const next = newerBoard(state.board, adaptBoard(wire))
        return next === state.board ? state : { board: next }
      })
    } finally {
      set({ busy: false })
    }
  },
}))
