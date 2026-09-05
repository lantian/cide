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
  attachments as attachmentsApi,
  tasks as tasksApi,
  type ProjectId,
  type TaskBoard as WireBoard,
  type TaskEdit,
  type TaskId,
  type TaskNew,
} from '@/ipc/client'
import { adaptBoard } from './TasksPanel/adapt'
import {
  BOARD_UNKNOWN,
  EMPTY_DRAFT,
  assigneeFromDraft,
  canWrite,
  changeFromDraft,
  isLinkKind,
  newerBoard,
  type AttachTargetView,
  type Board,
  type StagedAttachment,
  type TaskDraft,
} from './TasksPanel/model'

interface TasksStore {
  project: ProjectId | null
  /** What the tracker last said. **Never null** — see `BOARD_UNKNOWN`. */
  board: Board
  /** Which task the panel has open, or `null` for the list. */
  selected: TaskId | null
  /**
   * The task being composed, or `null` when the compose dialog is not up. (M21)
   *
   * # Why the half-typed draft is here and not in the host
   *
   * This was a `composing: boolean`, because *New task* used to create the row immediately and
   * the flag existed only to stop a second click writing a second one. The dialog replaced that
   * gesture, and the flag grew a payload rather than a companion: two fields — "is it open" and
   * "what is in it" — can disagree, and the state where the dialog is up with no draft is one
   * every reader would have to handle and none could produce anything sensible for.
   *
   * It is in the store for the reason the flag was: it must **survive the panel unmounting**.
   * The sidebar can be toggled shut, or switched to Files, in the middle of writing a task, and
   * a draft held in `TasksPanelHost`'s `useState` would be gone when it came back — which is the
   * silent data loss the whole dialog exists to prevent, arriving by a different door.
   *
   * It is emphatically **not** durable: nothing writes it to disk, nothing puts it on the wire,
   * and no other window can see it. `TaskDraft`'s own doc argues why that is allowed — the task
   * does not exist yet, so there is nothing for two windows to disagree about.
   */
  compose: TaskDraft | null
  /**
   * True from the moment Create is pressed until the write lands or fails.
   *
   * The dialog stays up and its Create goes inert. Closing on the click instead would be one
   * fewer state to carry and would throw the user's paragraph away on a write that failed —
   * there is no draft on disk to recover it from, which is the point of a draft that never
   * reached Rust.
   */
  creating: boolean

  /** Point the store at a project, or at nothing. Clears first. */
  attach: (project: ProjectId | null) => Promise<void>
  /** Ask Rust for the board again. The Retry button, and the tail of `attach`. */
  refresh: () => Promise<void>
  /** Take a board somebody else produced — a broadcast, or a mutation's answer. */
  adopt: (project: ProjectId, board: WireBoard) => void
  select: (task: TaskId | null) => void
  /** Open the compose dialog on an empty draft. A no-op while one is already open. */
  beginCompose: () => void
  /** A keystroke in the dialog. The draft is replaced whole; the dialog is controlled. */
  setDraft: (draft: TaskDraft) => void
  /** Discard the draft and close the dialog. Cancel, the ✕ and Escape all arrive here. */
  endCompose: () => void
  /**
   * Create a task, and with it `.cide/tasks.json` if the project has none.
   *
   * Takes the whole draft rather than `(title, body)`: the dialog collects four fields and a
   * create that dropped two of them would make the user open the card and set them again, which
   * is the two-writes-instead-of-one shape `TaskNew::status` was added to end.
   */
  create: (draft: TaskDraft) => Promise<void>
  edit: (task: TaskId, edit: TaskEdit) => Promise<void>
  remove: (task: TaskId) => Promise<void>
  /**
   * Files by path onto the body, a comment, or a new comment. (M39) The `edit` shape — the
   * board comes back and is adopted — for a different command, because attaching copies bytes
   * before it records anything and is not a `TaskEdit`.
   */
  attachFiles: (
    task: TaskId,
    target: AttachTargetView,
    sources: readonly StagedAttachment[],
  ) => Promise<void>
  /** The clipboard's image, straight onto the target. Resolves `false` when it held none. */
  attachClipboard: (task: TaskId, target: AttachTargetView) => Promise<boolean>
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
  compose: null,
  creating: false,

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
    // The draft goes with the selection, and for a sharper version of its reason: a half-written
    // task is *for* the project it was started in — its assignee names a role out of that
    // project's `.cide/`, and Create would write it into the tracker of whichever project the
    // user happens to be looking at when they press it.
    set({
      project,
      board: BOARD_UNKNOWN,
      selected: null,
      compose: null,
      creating: false,
    })
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

  /*
   * Idempotent, and that is the re-entrancy guard the boolean used to be: a second *New task*
   * while the dialog is up must not throw away what has been typed into it. `EMPTY_DRAFT` is a
   * module constant, so re-opening does not mint a fresh object either.
   */
  beginCompose: () => {
    if (get().compose !== null) return
    set({ compose: EMPTY_DRAFT })
  },
  setDraft: (draft) => set({ compose: draft }),
  endCompose: () => set({ compose: null, creating: false }),

  create: async (draft) => {
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
     * Built field by field, and each optional folded in only when it has a value: under
     * `exactOptionalPropertyTypes` an explicit `body: undefined` is not the same as an absent
     * `body`, and every one of these three is `Option<…>` in `TaskNew` precisely so the common
     * call — an orchestrator creating five titles at once — need not send three empty values
     * five times.
     *
     * `title` is trimmed here rather than in the dialog. `draftReady` already refuses a
     * whitespace-only one, so this is about the *inside* of the field: `TaskStore::create` trims
     * too, and a title that arrived with a trailing space would come back from Rust differing
     * from the draft that produced it.
     *
     * `status` is the field `TaskNew` grew for this dialog, and it is the whole reason the
     * segment in it is not decoration — see that field's doc for why it is the user's and
     * structurally not an agent's.
     */
    const body = draft.body.trim() === '' ? undefined : draft.body
    const agent = assigneeFromDraft(draft.assignee)

    /*
     * The change this task names, if it names one — an **existing** change and never a new one.
     *
     * This used to also *propose*: the picker offered "New change from this task", and this
     * branch called `spec_propose` before the create so the task could be born linked. What that
     * produced was a scaffold — a stub proposal, no delta specs, no checklist — which is a change
     * `openspec validate` refuses and a board row stuck at `0/0`. The link was correct and the
     * thing it linked to was broken.
     *
     * Proposing moved to `spec_propose_for_task`, which runs `/openspec-propose` in a
     * conversation and tells it to link the change back with `cide_task_update`. That inverts the
     * ordering argument this comment used to make, and the inversion is safe for the reason the
     * original was not: nothing is dispatched from a task with no change, so there is no run to
     * mislead — where a task born linked to an empty change would have dispatched a run at a
     * checklist that did not exist.
     */
    const change = changeFromDraft(draft)

    /*
     * The draft's links, folded in with the creation rather than as follow-up edits. (M30)
     * `TaskNew::links` carries the interleaving argument: a create naming an assignee
     * dispatches, and a blockedBy attached one call later is a gate the trigger never saw.
     * Through the `isLinkKind` guard rather than a cast — the dialog only offers the three,
     * so the filter narrows without ever dropping anything a user picked.
     */
    const links = draft.links.flatMap((link) =>
      isLinkKind(link.kind) ? [{ link: link.kind, target: link.target }] : [],
    )
    // The staged files, as paths — the only field of a `StagedAttachment` that crosses the seam.
    // On the create itself rather than as an attach afterwards, for `TaskNew::attachments`'
    // reason: one broadcast, with the files in it. (M39)
    const attachments = draft.attachments.map((file) => file.path)
    const req: TaskNew = {
      project,
      title: draft.title.trim(),
      ...(body === undefined ? {} : { body }),
      ...(agent === null ? {} : { agent }),
      ...(change === null ? {} : { change: change as never }),
      ...(links.length === 0 ? {} : { links }),
      ...(attachments.length === 0 ? {} : { attachments }),
      status: draft.status,
    }
    /*
     * The dialog stays up across the await, with Create inert. See `creating`: a dialog that
     * closed on the click would take the user's paragraph with it the first time a write failed,
     * and there is nothing anywhere to recover it from.
     */
    set({ creating: true })
    try {
      const wire = await tasksApi.create(req)
      // The user left for another project mid-write. The task was still created — in the project
      // it was composed for — so this is not a failure; there is simply nothing here to paint,
      // and `attach` has already cleared the dialog.
      if (get().project !== project) return
      get().adopt(project, wire)
      /*
       * Closed **only once the write landed**, and the card is deliberately *not* opened on top.
       *
       * *New task* used to create a row and open its card, because the card was where the task
       * got its title. The dialog is that surface now, so opening a second modal for a task
       * whose four fields the user has just filled in would be answering a finished gesture with
       * another one. The confirmation is the row appearing in the list, which is what
       * `filterAfterCreate` is for — a create the user cannot see reads as a create that did not
       * happen.
       */
      set({ compose: null })
    } finally {
      set({ creating: false })
    }
  },

  edit: async (task, edit) => {
    const project = get().project
    if (project === null) return
    const wire = await tasksApi.edit(project, task, edit)
    if (get().project !== project) return
    get().adopt(project, wire)
  },

  attachFiles: async (task, target, sources) => {
    const project = get().project
    if (project === null) return
    const wire = await attachmentsApi.attach(
      project,
      task,
      target,
      sources.map((file) => file.path),
    )
    if (get().project !== project) return
    get().adopt(project, wire)
  },

  attachClipboard: async (task, target) => {
    const project = get().project
    if (project === null) return false
    const wire = await attachmentsApi.attachClipboard(project, task, target)
    if (wire === null) return false
    if (get().project !== project) return true
    get().adopt(project, wire)
    return true
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
