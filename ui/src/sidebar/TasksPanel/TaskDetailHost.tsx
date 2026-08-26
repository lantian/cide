/**
 * The open task's card as the app mounts it: `TaskDetailModal`, plus the stores and the clock.
 *
 * ```tsx
 * {taskSelected !== null && (
 *   <PanelBoundary name="Task" …><TaskDetailHost /></PanelBoundary>
 * )}
 * ```
 *
 * # Why the card's mount left `TasksPanelHost`
 *
 * It used to live there, beside the list, which meant the card could only exist while the Tasks
 * panel was the sidebar view — and the Agents panel's task links (a run row's second line, and
 * the same chip in Recent) therefore had to *switch the sidebar to Tasks* before anything could
 * appear. The user asked for the smaller gesture: clicking a task under an agent opens the card
 * and moves nothing else. A modal already portals to `document.body` and dims the whole window;
 * yanking the panel out from under it was ceremony, not information.
 *
 * So the mount is `App.tsx`'s now, outside every `sidebar.view` branch, gated only on
 * `tasksStore.selected` — which has always been store state precisely so it survives the panel
 * unmounting. The card opens over whichever panel is showing, from whichever panel selected it,
 * and the Tasks panel keeps exactly the half that is about the *list*: the row of the open task
 * is still marked, because `selected` still reaches it.
 *
 * App gates the mount on `selected !== null` rather than this host returning `null` from inside
 * an always-mounted boundary, and the difference is the boundary: `PanelBoundary` never resets
 * itself, so its close gesture must *unmount* it — clearing the selection collapses the branch,
 * which is what arms the boundary for the next open. The same shape as every sidebar panel.
 *
 * # Why this file exists at all
 *
 * `TaskDetail.tsx` is rendered under node by `ui/scripts/check-agents-render.mjs`, through
 * `react-dom/server`, with nothing stubbed but `window`. That works because it reads **no
 * store, calls no IPC and never reads the clock** — every fact and every gesture arrives as a
 * prop. This host holds the half a server render cannot follow, exactly as `TasksPanelHost`
 * does for the list; its header states the rule at more length.
 *
 * # Two pieces of transient state moved here with the card
 *
 * The field the card has in edit, and the card's armed delete. Both are **transient gesture
 * state** — what is half-typed in this window's box, which button this window is looking at —
 * and both belong to whatever mounts the card, because they are claims about *this card's*
 * controls. `TasksPanelHost` keeps its own `deleteArmed` for the list's row-level delete; the
 * two armings no longer share a value, and that is fine rather than a loss: an arming is a
 * claim about the specific control the user pressed, and carrying it from a row into the card
 * was a coincidence of where the state happened to live, not a behaviour anything relied on.
 *
 * # `nowMs` is a prop, and the clock lives here
 *
 * The comment log prints ages ("4m ago"), so something has to tick — 30 s, `TasksPanelHost`'s
 * cadence and its reasoning: the only thing that moves faster is the seconds figure on a
 * comment under a minute old. Re-armed when the board changes, so a comment the user just
 * wrote reads `0s ago` immediately rather than inheriting the previous tick.
 *
 * # It reads both stores, like the two panel hosts it sits between
 *
 * `runs` and `roles` come from the agents roster — the assignee dropdown and the live-run
 * strip are the card's, so the join moved here with it. The same derivation `TasksPanelHost`
 * still does for the list, and `check-agents-render.mjs` greps both files for it: the seam is
 * the one neither render check mounts, and it shipped an empty-forever dropdown once already.
 */
import { memo, useCallback, useEffect, useMemo, useState } from 'react'
import { notifyFailure } from '@/chrome/notices'
import { useTasks } from '@/sidebar/tasksStore'
import { useAgents } from '@/sidebar/agentsStore'
import { rosterRoles } from '@/sidebar/AgentsPanel/model'
import { TaskDetailModal } from './TaskDetail'
import { activeEdit, armedDelete, assigneeHint, openTask } from './model'
import type { ArmedDelete, FieldEdit, RunRef } from './model'

/** How often the comment log's ages are recomputed. See the header for why it is not 1 s. */
const TICK_MS = 30_000

/**
 * Memoised like the panel hosts, and for their reason: it is a direct child of `App`, which
 * re-renders on every store notification it subscribes to, and everything the card draws
 * arrives through this host's own subscriptions.
 */
export const TaskDetailHost = memo(TaskDetailHostImpl)

function TaskDetailHostImpl() {
  const board = useTasks((s) => s.board)
  const selected = useTasks((s) => s.selected)
  const select = useTasks((s) => s.select)
  const editTask = useTasks((s) => s.edit)
  const removeTask = useTasks((s) => s.remove)

  /*
   * The join with the agents store — see the header. All four selectors return stored values
   * (`check:selectors`' rule); `roles` is derived, so it is memoised on the roster's identity,
   * which `adopt` replaces wholesale on every broadcast.
   */
  const roster = useAgents((s) => s.roster)
  const openRun = useAgents((s) => s.openPane)
  const pauseRun = useAgents((s) => s.pause)
  const resumeRun = useAgents((s) => s.resume)
  const roles = useMemo(() => rosterRoles(roster), [roster])
  /* `RunView` is structurally a `RunRef` — the five fields the chip reads, `phase` opaque. */
  const runs: readonly RunRef[] = roster.kind === 'ready' ? roster.runs : NO_RUNS
  const hint = assigneeHint(roster.kind)

  /**
   * The one field the card has in edit, and what has been typed into it.
   *
   * # Why the draft is here and not in the DOM
   *
   * The card's fields used to be uncontrolled — `defaultValue`, committing on blur — which was
   * right for a form and is wrong for a read-first card: *is this field dirty* is consulted by
   * three separate gestures (opening another field, the scrim, Escape), and a decision that
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
   * `deleteArmed` below is cleared on any board change because an arming is a claim about a
   * screen that has been replaced. A draft is not: an agent appending a comment to the task
   * somebody is retitling must not take the title away from them. What *does* clear it is the
   * card closing or opening on a different task — including the store clearing `selected` when
   * the open task is deleted by another writer, which is the case this effect exists for.
   * `activeEdit` covers the frame before the effect runs, for the reason `armedDelete`'s pure
   * gate does.
   */
  const [editing, setEditing] = useState<FieldEdit | null>(null)
  useEffect(() => setEditing(null), [selected])

  /**
   * The delete the user has armed **in the card**, and the board they armed it against. The
   * list's row-level arming is `TasksPanelHost`'s own — see the header for why the split is
   * deliberate. Disarmed whenever the board moves, because an arming is a claim about a screen
   * that has been replaced; `model.ts::armedDelete` refuses the stale pair on the frame before
   * this effect runs.
   */
  const [deleteArmed, setDeleteArmed] = useState<ArmedDelete | null>(null)
  useEffect(() => setDeleteArmed(null), [board])

  const [nowMs, setNowMs] = useState(() => Date.now())
  useEffect(() => {
    setNowMs(Date.now())
    const timer = setInterval(() => setNowMs(Date.now()), TICK_MS)
    return () => clearInterval(timer)
    // `board` on purpose: a new comment should be `0s ago` on the frame it appears, not on the
    // next tick. `newerBoard` keeps the identity stable across dropped snapshots, so a
    // broadcast that changes nothing re-arms nothing.
  }, [board])

  /**
   * Every mutation goes out the same way: fire it, and show the reason if it fails. A gesture
   * that silently does nothing is the failure this project has paid for most often;
   * `TasksPanelHost`'s copy says why the surfacing lives at the gesture and not in the store.
   */
  const guarded = useCallback((done: Promise<void>) => {
    void done.catch(notifyFailure)
  }, [])

  /*
   * The open task, and the edit that still applies to it. `openTask` returns `null` on a board
   * that is not `ready`, which is what keeps the card off screen over an unparseable tracker —
   * and off screen in a window whose board was never attached at all.
   */
  const open = openTask(board, selected)
  const edit = activeEdit(board, open, editing)

  if (open === null) return null
  return (
    <TaskDetailModal
      /*
       * Keyed on the task id. Nothing in the card is uncontrolled any more except the comment
       * composer — and that is precisely what the key is for: half a comment aimed at `t-14`
       * must not still be in the box when `t-15` opens.
       */
      key={open.id}
      task={open}
      runs={runs}
      roles={roles}
      assigneeHint={hint}
      onOpenRun={(run) => guarded(openRun(run))}
      onPauseRun={(run) => guarded(pauseRun(run))}
      onResumeRun={(run) => guarded(resumeRun(run))}
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
  )
}

/*
 * The non-`ready` fallback for `runs`, module-level for the reason `TasksPanelHost`'s copy is:
 * a fresh `[]` per render would be a new prop identity for a value that has not changed.
 */
const NO_RUNS: readonly RunRef[] = []
