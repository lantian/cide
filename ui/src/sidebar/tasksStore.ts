/**
 * The project's task board, and what keeps it fresh. (M18)
 *
 * Modelled on `diagnosticsStore` line for line, because it answers the same shape of question: a
 * per-project snapshot that several surfaces read — the panel, and the activity rail's ☑ badge —
 * refreshed on an event rather than on a timer.
 *
 * # Every write path goes through `adopt`, and `adopt` goes through `newerBoard`
 *
 * That is the load-bearing sentence in this file. `.cide/tasks.json` genuinely has several
 * writers — two cide windows, and every dispatched agent through the orchestration MCP server —
 * so a snapshot can arrive out of order: a `tasks_board` answer that was slow, landing after a
 * `cide://tasks-changed` broadcast that already carried the newer state. Painting the slow one
 * shows a board behind the click that caused it. `TasksPanel/model.ts::newerBoard` is that drop
 * rule, extracted as a pure function so `check:agents` can drive it, and it is called from
 * exactly one place: here.
 *
 * `newerBoard` returns the **identical object** when it drops a snapshot, so a reader selecting
 * `s.board` sees the same reference and does not re-render. Nothing here may defeat that by
 * wrapping the result in a fresh object, and nothing may `set` unconditionally — a `set` of an
 * identical value still runs every subscriber's selector, and the one thing this store must not
 * do is repaint a list the user is mid-scroll in on every heartbeat from an agent.
 *
 * # Why there is no `schedule`
 *
 * `diagnosticsStore` coalesces because a diagnostics push is a *hint* that costs a round trip to
 * act on. `cide://tasks-changed` carries the whole board, so acting on it costs nothing and
 * there is nothing to coalesce: the handler adopts the payload and never asks Rust anything.
 * `refresh` exists for the two moments that have no payload — attaching to a project, and the
 * Retry button on an unreadable file.
 *
 * # Sessions of this store outlive the panel
 *
 * The subscription lives in `App.tsx` and not in `TasksPanelHost`, for `gitCountStore`'s stated
 * reason: the rail's badge has to stay live while the sidebar is shut or showing Files, and a
 * listener registered inside a panel goes stale the moment that panel unmounts.
 */
import { create } from 'zustand'
import {
  tasks as tasksApi,
  type ProjectId,
  type TaskBoard as WireBoard,
  type TaskEdit,
  type TaskId,
  type TaskNew,
} from '@/ipc/client'
import { adaptBoard } from './TasksPanel/adapt'
import { BOARD_UNKNOWN, canWrite, newerBoard, type Board } from './TasksPanel/model'

interface TasksStore {
  project: ProjectId | null
  /** What the tracker last said. **Never null** — see `BOARD_UNKNOWN`. */
  board: Board
  /** Which task the panel has open, or `null` for the list. */
  selected: TaskId | null
  /**
   * Whether a create gesture is under way.
   *
   * Owned by the store rather than by the host because it must survive the panel unmounting —
   * the sidebar can be toggled shut mid-round-trip — and because it is what stops a second
   * click on *New task* from putting a second task in a file the team commits.
   */
  composing: boolean

  /** Point the store at a project, or at nothing. Clears first. */
  attach: (project: ProjectId | null) => Promise<void>
  /** Ask Rust for the board again. The Retry button, and the tail of `attach`. */
  refresh: () => Promise<void>
  /** Take a board somebody else produced — a broadcast, or a mutation's answer. */
  adopt: (project: ProjectId, board: WireBoard) => void
  select: (task: TaskId | null) => void
  beginCompose: () => void
  endCompose: () => void
  /** Create a task, and with it `.cide/tasks.json` if the project has none. */
  create: (title: string, body?: string) => Promise<void>
  edit: (task: TaskId, edit: TaskEdit) => Promise<void>
  remove: (task: TaskId) => Promise<void>
}

/**
 * Claimed *before* each call and re-checked after, so an answer for a project the user has since
 * left is dropped rather than painted. The same guard `diagnosticsStore` and `gitStatusStore`
 * use, and the same reason: `attach` is called from a render effect and can be re-entered before
 * the previous call lands.
 */
let generation = 0

export const useTasks = create<TasksStore>((set, get) => ({
  project: null,
  board: BOARD_UNKNOWN,
  selected: null,
  composing: false,

  attach: async (project) => {
    generation += 1
    /*
     * Cleared to `BOARD_UNKNOWN` **before the await**, not left standing. The previous project's
     * tasks over the new project's name is a *wrong* answer that lasts a frame and reads as
     * real; "nobody has looked" is a true one that lasts one round trip and draws nothing at
     * all. `Board`'s fourth arm exists for precisely this window — see its doc comment, which
     * argues why `absent` must not be the placeholder: it names a path and puts a button that
     * creates a file under it.
     *
     * The selection goes with it. A `t-14` opened in the previous project is not a task in this
     * one, and an id that happens to collide would open a different task under the same name.
     */
    set({ project, board: BOARD_UNKNOWN, selected: null, composing: false })
    if (project === null) return
    await get().refresh()
  },

  refresh: async () => {
    const project = get().project
    if (project === null) return
    generation += 1
    const mine = generation

    /*
     * `tasks.board` goes through `pendingCommand` inside `client.ts`, so a build whose backend
     * has no `tasks_board` handler answers `null` rather than rejecting — this is called from a
     * render effect, and an unhandled rejection out of an effect unmounts the tree under React
     * 19. `null` is *"this build cannot answer"*, which is the same **state** as "the answer is
     * not in yet" and is therefore `BOARD_UNKNOWN`; it is emphatically not `absent`, which would
     * claim the user's project has no tracker on the strength of a missing command.
     */
    const wire = await tasksApi.board(project)
    // Both guards, in this order: a newer call has superseded this one, or the user moved to
    // another project while it was in flight. Either alone lets a stale answer through.
    if (mine !== generation || get().project !== project) return
    if (wire === null) {
      set({ board: BOARD_UNKNOWN })
      return
    }
    get().adopt(project, wire)
  },

  adopt: (project, wire) => {
    // A broadcast for a project this window is not showing. Every window hears every emit.
    if (get().project !== project) return
    const held = get().board
    const next = newerBoard(held, adaptBoard(wire))
    // `newerBoard` hands back the board it was given when it drops the snapshot. Setting it
    // anyway would run every subscriber's selector for nothing; see the header.
    if (next === held) return
    set({ board: next })
  },

  select: (task) => set({ selected: task }),

  beginCompose: () => set({ composing: true }),
  endCompose: () => set({ composing: false }),

  create: async (title, body) => {
    const project = get().project
    if (project === null) return
    /*
     * **The write gate, and it lives here rather than in the panel.** `canWrite` is `false` for
     * an `unreadable` board — an app that "recovers" from an unparseable `.cide/tasks.json` by
     * writing over it has destroyed the user's data to fix its own display, and the likeliest
     * cause of one is a half-resolved merge conflict, which is to say a file still containing
     * both sides of everything. It is `false` for `unknown` too: nobody has looked, so the write
     * would be composed against a board that may not be the one on disk.
     *
     * `TasksPanel.tsx` asks the same question to decide whether to *draw* the button, which is a
     * different question with the same answer — one rule, two callers, and no second copy of it.
     * This is the copy that is load-bearing, because a palette `task.new` will not go through
     * the panel at all.
     */
    if (!canWrite(get().board)) return
    /*
     * Built rather than spread, and `body` is folded in only when it is a string: under
     * `exactOptionalPropertyTypes` an explicit `body: undefined` is not the same as an absent
     * `body`, and `TaskNew::body` is `Option<String>` precisely so the common call need not send
     * an empty string.
     */
    const req: TaskNew = { project, title, ...(body === undefined ? {} : { body }) }
    /*
     * Which ids existed before the call, so the new one can be found in the answer. The board
     * comes back whole — no `hydrate()`, no second ask — and Rust does not say which task it
     * minted, because the file is a list whose order is the priority and "the last one" is not a
     * promise the format makes.
     */
    const before = new Set(idsOf(get().board))
    const wire = await tasksApi.create(req)
    if (get().project !== project) return
    get().adopt(project, wire)
    const minted = idsOf(get().board).filter((id) => !before.has(id))
    /*
     * Opened only when exactly one task is new. Two means another writer landed a task in the
     * same window — an agent, or the other cide window — and opening the wrong one is worse than
     * opening none: the user would start typing a title into somebody else's task.
     */
    const first = minted[0]
    if (minted.length === 1 && first !== undefined) set({ selected: first })
  },

  edit: async (task, edit) => {
    const project = get().project
    if (project === null) return
    const wire = await tasksApi.edit(project, task, edit)
    if (get().project !== project) return
    get().adopt(project, wire)
  },

  remove: async (task) => {
    const project = get().project
    if (project === null) return
    const wire = await tasksApi.remove(project, task)
    if (get().project !== project) return
    get().adopt(project, wire)
    // The detail view falls back to the list on its own when the selected id is not in the
    // board, but leaving it set would re-open the task if an out-of-order snapshot briefly
    // brought it back. The selection is this store's, so it is cleared here.
    if (get().selected === task) set({ selected: null })
  },
}))

/** The ids a board holds, or `[]` for every arm that holds none. */
function idsOf(board: Board): readonly TaskId[] {
  return board.kind === 'ready' ? board.tasks.map((task) => task.id) : []
}
