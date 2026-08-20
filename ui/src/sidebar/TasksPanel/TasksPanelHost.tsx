/**
 * The Tasks panel as the app mounts it: `TasksPanelView`, plus the store and the clock. (M18)
 *
 * ```tsx
 * {sidebar.view === 'tasks' && <TasksPanel project={activeProjectId} />}
 * ```
 *
 * # It mounts two things, and that is the shape of the change that made the card a modal
 *
 * `TasksPanelView` — the list, always — and, when a task is open, `TaskDetailModal` **beside**
 * it rather than in place of it. The card is an `OverlayCard`, which portals to `document.body`,
 * so it is not clipped by the 320px sidebar, is not capped by the tab panel's stacking context,
 * and leaves the board on screen behind its scrim. The list therefore never disappears while a
 * task is open, which is what makes the row-marking in the view worth drawing.
 *
 * The pair is mounted from here rather than from inside the view for the reason the whole file
 * exists: a portal cannot be server-rendered — `react-dom/server` throws on one — and rendering
 * the card from `TasksPanel.tsx` would take every board state, every group and both empty
 * screens out of `check-agents-render.mjs` along with it.
 *
 * # Why this file exists at all
 *
 * `TasksPanel.tsx` and `TaskDetail.tsx` are rendered under node by
 * `ui/scripts/check-agents-render.mjs`, through `react-dom/server`, with nothing stubbed but
 * `window` — which is the only gate in this project that can see a panel that compiles, mounts
 * and draws nothing. That works because those two files read **no store, call no IPC and never
 * read the clock**: every fact and every gesture arrives as a prop. `GitPanelHost` exists for
 * the same reason and states it at more length.
 *
 * So this file holds the half a server render cannot follow, and it must stay the only one:
 * a `useTasks` call moved down into the view would drag `@/ipc/client` — and with it
 * `@tauri-apps/api` — into a check whose whole value is that it needs neither.
 *
 * # `nowMs` is a prop, and the clock lives here
 *
 * The comment log prints ages ("4m ago"), so something has to tick. A `Date.now()` inside the
 * view would make its markup differ between two renders of the same fixture and the render
 * check could not digest it at all. The interval is 30 s rather than 1 s because the only thing
 * that moves faster than that is the seconds figure on a comment less than a minute old, and a
 * per-second repaint of a sidebar list for that is not a trade worth making. It is also re-armed
 * whenever the board changes, so a comment the user just wrote reads `0s ago` immediately rather
 * than inheriting the previous tick.
 *
 * # `runs` is empty, deliberately, until the Agents slice lands
 *
 * `agentChip` takes the runs array and answers one of three things: a **live** run on this task
 * (lit, with its phase dot), the **assigned** role (dim), or nothing. With no runs it can only
 * ever answer the second or the third — which is the *correct* rendering of what this build
 * knows, not a stub: a task assigned to `developer` says so quietly, and cide never claims an
 * agent is working on it. `model.ts` says the same thing in its header. **Do not "fix" this by
 * inventing a run**; the fix is `agentsStore`, and when it lands this prop and `roles` are the
 * two lines that change.
 *
 * `roles` is empty for the same reason — it is the roster's `AgentDef.label` map. `agentChip`
 * falls back to printing the raw agent id, so an unassigned-role chip still reads correctly.
 *
 * # Three pieces of state live here rather than in `tasksStore`
 *
 * The status filter, the armed delete, and the field the card has in edit. All three are
 * **transient gesture state** — which overlay is open, which button this window is looking at,
 * what is half-typed in this window's box — and that is the one category `docs/adr/0002` leaves
 * to the webview. None is durable, and none should reach another window: a second cide window
 * showing the same project must not find its list filtered, a delete armed, or a title field
 * open with somebody else's half-sentence in it.
 *
 * The store owns `selected` and `compose` instead, and both for the same reason, stated in their
 * own docs: they must **survive this panel unmounting**. The sidebar can be toggled shut or
 * switched to Files, and a task half-written into a `useState` here would be gone when it came
 * back — which is the silent data loss the compose dialog exists to prevent, arriving by a
 * different door.
 */
import { useCallback, useEffect, useState } from 'react'
import { fsReveal, type ProjectId } from '@/ipc/client'
import { notifyFailure } from '@/chrome/notices'
import { useTasks } from '@/sidebar/tasksStore'
import { TasksPanelView } from './TasksPanel'
import { TaskDetailModal } from './TaskDetail'
import { TaskComposeModal } from './TaskCompose'
import { activeEdit, armedDelete, filterAfterCreate, openTask } from './model'
import type { ArmedDelete, FieldEdit, RunRef, StatusFilter } from './model'

/** How often the comment log's ages are recomputed. See the header for why it is not 1 s. */
const TICK_MS = 30_000

export interface TasksPanelProps {
  /** The active project, or `null` — in a detached-pane window, and before one is open. */
  project: ProjectId | null
}

export function TasksPanel({ project }: TasksPanelProps) {
  const board = useTasks((s) => s.board)
  const selected = useTasks((s) => s.selected)
  const compose = useTasks((s) => s.compose)
  const creating = useTasks((s) => s.creating)
  const select = useTasks((s) => s.select)
  const refresh = useTasks((s) => s.refresh)
  const beginCompose = useTasks((s) => s.beginCompose)
  const setDraft = useTasks((s) => s.setDraft)
  const endCompose = useTasks((s) => s.endCompose)
  const createTask = useTasks((s) => s.create)
  const editTask = useTasks((s) => s.edit)
  /*
   * `task_delete`'s only caller. It has been a registered command with a `client.ts` wrapper and
   * a working store action since M18 and **nothing rendered a control that reached it** — the
   * twenty-second time this project has shipped something built and reachable from nothing (pause
   * and resume were #21, and `README.md` keeps the count). The store already clears `selected`
   * when the deleted task was the open one, so the card falls back to the list on its own.
   */
  const removeTask = useTasks((s) => s.remove)

  /**
   * Which status the list is narrowed to, or `null` for the whole board.
   *
   * # It survives a project switch, deliberately
   *
   * `attach` clears the board *and* `selected`, and this is not cleared with them. The
   * difference is what the value refers to. `selected` is a **task id**, which belongs to one
   * project — `t-14` in the project you left is a different task from `t-14` here, and an id
   * that happened to collide would silently open the wrong one. A status is not project-scoped:
   * every board has the same four, so the filter cannot dangle and cannot mean something else
   * after the switch. It is a way of *reading* a board, like the sidebar's width or the Done
   * group being collapsed, and resetting those on every project switch is how a preference
   * becomes something the user has to keep re-setting.
   *
   * The risk that makes this worth arguing rather than assuming: arriving in a new project with
   * a filter on could look like an empty tracker. It cannot, because `listEmpty` splits those
   * two states and the filtered-to-nothing screen names the filter, counts the tasks that exist
   * and offers Show all — and the filter row itself stays on screen, lit on the one that is on.
   */
  const [filter, setFilter] = useState<StatusFilter>(null)

  /**
   * The delete the user has armed, and the board they armed it against.
   *
   * `AgentsPanelHost`'s `integrateArmed`, one panel over, and the same reasoning: the act is
   * recoverable — `.cide/tasks.json` is committed, so `git log -p` has the task and `git
   * checkout` brings it back — which makes a modal heavier than it is worth and a single click
   * lighter than it is worth. Confirm-on-second-click is the weight that fits.
   *
   * It carries the `rev` as well as the id because a row-level delete in a list is a mis-click
   * waiting to happen, and this file has several writers: two cide windows and every dispatched
   * agent. `model.ts::armedDelete` is what refuses an arming whose board has moved, and it is a
   * *pure* function applied on every render precisely because the effect below runs after paint
   * — one frame in which the new board is on screen under the old arming is one frame in which a
   * click confirms against something the user never saw.
   */
  const [deleteArmed, setDeleteArmed] = useState<ArmedDelete | null>(null)

  /**
   * The one field the card has in edit, and what has been typed into it.
   *
   * # Why the draft is here and not in the DOM
   *
   * The card's fields used to be uncontrolled — `defaultValue`, committing on blur — which was
   * right for a form and is wrong for a read-first card: *is this field dirty* is now consulted
   * by three separate gestures (opening another field, the scrim, Escape), and a decision that
   * reads the DOM cannot be driven by `check-agents.mjs`. `model.ts` decides all three from this
   * value, and the component executes what it is handed.
   *
   * The objection the old comment raised against controlled fields does not apply. It was about
   * a field controlled *by the board* — `value={task.title}` — which a `tasks-changed` landing
   * mid-sentence would overwrite. This is the user's own text, derived from nothing, so a
   * snapshot cannot touch it; `activeEdit` is what decides when it stops applying, and a new
   * `rev` deliberately is not one of those times.
   *
   * # Cleared when the open task changes, and not when the board does
   *
   * `armedDelete` is cleared on any board change because an arming is a claim about a screen
   * that has been replaced. A draft is not: an agent appending a comment to the task somebody is
   * retitling must not take the title away from them. What *does* clear it is the card closing
   * or opening on a different task — including the store clearing `selected` when the open task
   * is deleted by another writer, which is the case this effect exists for. `activeEdit` covers
   * the frame before the effect runs, for the reason `armedDelete`'s pure gate does.
   */
  const [editing, setEditing] = useState<FieldEdit | null>(null)
  useEffect(() => setEditing(null), [selected])

  /*
   * Disarm whenever the board moves, so a stale arming does not sit in state waiting for a `rev`
   * to come round again. `board` is the store's object identity, and `newerBoard` deliberately
   * hands back the identical object when it drops a snapshot — so an agent's heartbeat that
   * changes nothing disarms nothing, and only a real change does.
   */
  useEffect(() => setDeleteArmed(null), [board])

  const [nowMs, setNowMs] = useState(() => Date.now())
  useEffect(() => {
    setNowMs(Date.now())
    const timer = setInterval(() => setNowMs(Date.now()), TICK_MS)
    return () => clearInterval(timer)
    // `board` on purpose: a new comment should be `0s ago` on the frame it appears, not on the
    // next tick. It is the store's object identity, and `newerBoard` deliberately keeps that
    // identical when it drops a snapshot, so a broadcast that changes nothing re-arms nothing.
  }, [board])

  /*
   * `.cide/tasks.json`'s path, which only the two screens that print it have. `undefined`
   * elsewhere, so the view draws no Reveal button rather than a dead one — `TaskLine`'s
   * `rowTag` rule applied to an action.
   */
  const path = board.kind === 'absent' || board.kind === 'unreadable' ? board.path : null

  /**
   * Every mutation goes out the same way: fire it, and show the reason if it fails.
   *
   * **Not** wrapped in `pendingCommand`, and the asymmetry with `tasks.board` is the design.
   * These are user gestures — a click on a status segment, a comment typed into the log — and a
   * gesture that silently does nothing is the failure this project has paid for most often.
   * `notifyFailure` puts the reason on screen through `chrome/Failures.tsx`; the store
   * deliberately does not import that, so the surfacing stays at the gesture, where a reader
   * looking at the click can find it.
   */
  const guarded = useCallback((done: Promise<void>) => {
    void done.catch(notifyFailure)
  }, [])

  /*
   * The open task, and the edit that still applies to it — both resolved **once**, here, so the
   * list's row marking and the card cannot disagree about which task is open, and so the pure
   * gates run on every render rather than only in the effect above. `openTask` returns `null` on
   * a board that is not `ready`, which is what keeps the card off screen over an unparseable
   * tracker without a second `canWrite` call.
   */
  const open = openTask(board, selected)
  const edit = activeEdit(board, open, editing)

  return (
    <>
      <TasksPanelView
        project={project}
        board={board}
        /* See the header: empty is the honest value until the Agents store exists. */
        runs={NO_RUNS}
        roles={NO_ROLES}
        selected={selected}
        filter={filter}
        onFilter={setFilter}
        deleteArmed={deleteArmed}
        /*
         * Arming reads `board.rev` from *this* render — the same board object the view below was
         * handed, so the rev recorded is the rev of the screen the user clicked on. Reading it
         * from the store at click time instead would record whatever had landed in the meantime,
         * which is the exact staleness the pair exists to detect.
         */
        onDeleteArm={(task) => {
          if (board.kind !== 'ready') return
          setDeleteArmed({ task, rev: board.rev })
        }}
        onDelete={(task) => {
          setDeleteArmed(null)
          guarded(removeTask(task))
        }}
        onSelectTask={select}
        /*
         * Passed unconditionally, and **not** gated on `canWrite` here. The view asks that
         * question once, at the top of its render, and decides whether the button exists at all;
         * asking it a second time in the host would be the second copy of the rule its own comment
         * warns about, and the copies are what drift. The *data* is protected a layer lower, in
         * `tasksStore.create`, which is where a palette `task.new` would arrive too.
         */
        onCreateTask={beginCompose}
        {...(project !== null && path !== null
          ? {
              onReveal: () => {
                // May legitimately fail on an `absent` board: there is nothing to reveal if
                // `.cide/` does not exist yet either. The failure is shown rather than swallowed —
                // a Reveal that does nothing looks like a broken button.
                void fsReveal.showInManager(project, path).catch(notifyFailure)
              },
            }
          : {})}
        onRetry={() => void refresh()}
      />
      {/*
        * The compose dialog, and the whole of the *New task* gesture. (M21)
        *
        * Nothing is written until Create: `onCreateTask` above opens this and does not touch
        * Rust, and the draft lives in the store — see `tasksStore.compose` for why it is there
        * and not in a `useState` here (the sidebar can be shut mid-sentence).
        *
        * `filterAfterCreate` runs **after** the write resolves, not on the click, because a
        * create that failed must not move the list the user is reading. What it prevents is the
        * create that reads as nothing happening at all: filter to Doing, create a Todo task, and
        * the row lands in a group the screen is not drawing — the dialog closes and there is no
        * evidence anywhere except a file nobody is looking at. It clears the filter rather than
        * switching it to the new task's status, so the row appears *and* everything the user had
        * chosen to look at is still there.
        */}
      {compose !== null && (
        <TaskComposeModal
          draft={compose}
          roles={NO_ROLES}
          busy={creating}
          onDraft={setDraft}
          onCancel={endCompose}
          onCreate={(draft) => {
            guarded(
              createTask(draft).then(() => {
                setFilter((current) => filterAfterCreate(current, draft.status))
              }),
            )
          }}
        />
      )}
      {/*
        * The card. Mounted beside the list, never instead of it — see the header.
        *
        * `runs` and `roles` are the same empty constants the list gets, for the same stated
        * reason, and `onOpenRun`/`onPauseRun`/`onResumeRun` are deliberately absent: each acts on
        * a run id, and with no runs no chip is ever lit, so the live-run strip that carries them
        * is never drawn. Passing handlers nothing can reach would be three more commands with no
        * caller; passing ones that did nothing would be worse.
        *
        * The five writes are each one `TaskEdit` variant — an enum rather than a patch of
        * options, for the reason its Rust doc gives: "assign to nobody" under a patch would need
        * `Option<Option<AgentId>>`, which serialises as two spellings of the same absence. The
        * card sends exactly one of them per gesture, which is what makes the per-field posture
        * fit the wire rather than fight it.
        */}
      {open !== null && (
        <TaskDetailModal
          /*
           * Keyed on the task id. Nothing in the card is uncontrolled any more except the comment
           * composer — and that is precisely what the key is for now: half a comment aimed at
           * `t-14` must not still be in the box when `t-15` opens.
           */
          key={open.id}
          task={open}
          runs={NO_RUNS}
          roles={NO_ROLES}
          nowMs={nowMs}
          editing={edit}
          onEditing={setEditing}
          onClose={() => select(null)}
          deleteArmed={armedDelete(board, deleteArmed) === open.id}
          onDeleteArm={(task) => {
            if (board.kind !== 'ready') return
            setDeleteArmed({ task, rev: board.rev })
          }}
          onDelete={(task) => {
            setDeleteArmed(null)
            guarded(removeTask(task))
          }}
          onSetTitle={(task, title) => guarded(editTask(task, { kind: 'setTitle', title }))}
          onSetStatus={(task, status) => guarded(editTask(task, { kind: 'setStatus', status }))}
          onSetAssignee={(task, agent) => guarded(editTask(task, { kind: 'assign', agent }))}
          onSetBody={(task, body) => guarded(editTask(task, { kind: 'setBody', body }))}
          onAddComment={(task, text) => guarded(editTask(task, { kind: 'comment', text }))}
          /* The author is not sent and cannot be: `task_edit` passes `TaskAuthor::User`, decided
             by the command rather than by this payload, which is what makes the Rust guard
             unforgeable rather than merely checked. See `TaskComment`. */
          onEditComment={(task, comment, text) =>
            guarded(editTask(task, { kind: 'editComment', id: comment, text }))
          }
          onDeleteComment={(task, comment) =>
            guarded(editTask(task, { kind: 'deleteComment', id: comment }))
          }
        />
      )}
    </>
  )
}

/*
 * Module-level constants rather than fresh literals in the JSX. `TasksPanelView` re-renders on
 * every tick of the clock above, and a new `[]` each time would be a new prop identity for a
 * value that has not changed — harmless today, and exactly the thing that stops being harmless
 * the moment anything downstream memoises on it.
 */
const NO_RUNS: readonly RunRef[] = []
const NO_ROLES: Readonly<Record<string, string>> = {}
