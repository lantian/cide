/**
 * The Tasks panel as the app mounts it: `TasksPanelView`, plus the store and the clock. (M18)
 *
 * ```tsx
 * {sidebar.view === 'tasks' && <TasksPanel project={activeProjectId} />}
 * ```
 *
 * # The open task's card is NOT mounted here any more
 *
 * It was — `TaskDetailModal`, beside the list — and that tied the card's existence to the Tasks
 * panel being the sidebar view, which forced the Agents panel's task links to switch the sidebar
 * before anything could open. The card is `TaskDetailHost`'s now, mounted from `App.tsx` outside
 * every `sidebar.view` branch, so a task opened from a run row appears over whatever panel is
 * showing. Its header carries the argument; what stays here is the list's half of the same
 * gesture — `selected` still reaches the view, so the open task's row is still marked behind
 * the card's scrim.
 *
 * The compose dialog is still mounted from here rather than from inside the view, for the
 * reason the whole file exists: a portal cannot be server-rendered — `react-dom/server` throws
 * on one — and rendering it from `TasksPanel.tsx` would take every board state, every group and
 * both empty screens out of `check-agents-render.mjs` along with it.
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
 * # The clock left with the card
 *
 * The 30 s tick this host used to run fed exactly one reader: the comment log's ages, which
 * are the card's. The list itself prints no relative time, so the tick moved to
 * `TaskDetailHost` and this host reads the clock nowhere — which also means the list stops
 * re-rendering twice a minute for a figure it never drew.
 *
 * # `runs` and `roles` come from the agents store — the join this host was written waiting for
 *
 * This header used to spend a paragraph on "`runs` is empty, deliberately, until the Agents
 * slice lands", and the promised two-line change is now real: the host subscribes
 * `useAgents((s) => s.roster)` and derives both props from it. `AgentsPanelHost` reaches the
 * other way (it reads `useTasks` for its task titles) for the same reason — the two stores are
 * two facts about one project, and each panel joins them at render. The years the paragraph
 * was a promise are also why `check-agents-render.mjs` now greps this file for the
 * subscription: the seam is exactly the one neither check mounts, and it shipped an
 * empty-forever assignee dropdown once already.
 *
 * With a roster that is not `ready` — disabled, empty, or nobody-has-looked — the props fall
 * back to the empty constants, which is the *correct* rendering of what this window knows:
 * `agentChip` answers the assigned role (dim) or nothing, and cide never claims an agent is
 * working. `assigneeHint` is the sentence under the dropdown that keeps that emptiness from
 * reading as the old bug.
 *
 * # Three pieces of state live here rather than in `tasksStore`
 *
 * The status filter, the search query, and the armed delete for the list's row-level control.
 * All three are **transient gesture state** — which overlay is open, which button this window
 * is looking at, what is half-typed in this window's box — and that is the one category
 * `docs/adr/0002` leaves to the webview. None is durable, and none should reach another window:
 * a second cide window showing the same project must not find its list filtered or searched, or
 * a delete armed. (The field the card has in edit moved to `TaskDetailHost` with the card — it
 * is a claim about the card's controls, so it belongs to whatever mounts the card.)
 *
 * The store owns `selected` and `compose` instead, and both for the same reason, stated in their
 * own docs: they must **survive this panel unmounting**. The sidebar can be toggled shut or
 * switched to Files, and a task half-written into a `useState` here would be gone when it came
 * back — which is the silent data loss the compose dialog exists to prevent, arriving by a
 * different door.
 */
import { memo, useCallback, useEffect, useMemo, useState } from 'react'
import {
  attachments as attachmentsApi,
  tasks as tasksApi,
  fsReveal,
  type ProjectId,
} from '@/ipc/client'
import { notify, notifyFailure } from '@/chrome/notices'
import { onFileDrop, useDropHot } from './fileDrop'
import { basename, type StagedAttachment } from './model'
import { useTasks } from '@/sidebar/tasksStore'
import { useSpec } from '../specStore'
import { useAgents } from '@/sidebar/agentsStore'
import { rosterRoles } from '@/sidebar/AgentsPanel/model'
import { TasksPanelView } from './TasksPanel'
import { TaskComposeModal } from './TaskCompose'
import { assigneeHint, filterAfterCreate, linkableTargets, queryAfterCreate } from './model'
import type { ArmedDelete, RunRef, StatusFilter } from './model'

export interface TasksPanelProps {
  /** The active project, or `null` — in a detached-pane window, and before one is open. */
  project: ProjectId | null
}

/**
 * Memoised, like every sidebar panel host: the panel is a direct child of `App`, which
 * re-renders on every store notification it subscribes to, and everything this panel draws
 * arrives through its own store subscriptions or through props `App` pins with `useCallback`
 * for exactly this. Without the memo, every App render re-walked the panel's full
 * unvirtualized row list for events that had nothing to do with it.
 */
export const TasksPanel = memo(TasksPanelImpl)

function TasksPanelImpl({ project }: TasksPanelProps) {
  const board = useTasks((s) => s.board)
  const selected = useTasks((s) => s.selected)
  const compose = useTasks((s) => s.compose)
  const creating = useTasks((s) => s.creating)
  const select = useTasks((s) => s.select)
  const refresh = useTasks((s) => s.refresh)
  const beginCompose = useTasks((s) => s.beginCompose)

  /*
   * Attachments in the New task dialog. (M39) The picker and the clipboard are promises the
   * dialog awaits and folds into the draft; a desktop drop on the dialog folds in the same way,
   * through the store rather than through a prop, because the drop arrives from outside React.
   */
  const dropHot = useDropHot()
  const pickAttachments = useCallback(async (): Promise<readonly StagedAttachment[]> => {
    const paths = await attachmentsApi.pick()
    return paths.map((path) => ({ path, name: basename(path), bytes: null }))
  }, [])
  const stageClipboard = useCallback(async (): Promise<StagedAttachment | null> => {
    const staged = await attachmentsApi.stageClipboard()
    if (staged === null) {
      notify('The clipboard holds no image.', { kind: 'warn' })
      return null
    }
    return { path: staged.path, name: staged.name, bytes: Number(staged.bytes) }
  }, [])
  useEffect(
    () =>
      onFileDrop((target, paths) => {
        if (target.kind !== 'compose') return
        const draft = useTasks.getState().compose
        if (draft === null) return
        const files = paths.map((path) => ({ path, name: basename(path), bytes: null }))
        useTasks.getState().setDraft({ ...draft, attachments: [...draft.attachments, ...files] })
      }),
    [],
  )
  const setDraft = useTasks((s) => s.setDraft)
  const endCompose = useTasks((s) => s.endCompose)
  const createTask = useTasks((s) => s.create)
  /*
   * `task_delete`'s only caller. It has been a registered command with a `client.ts` wrapper and
   * a working store action since M18 and **nothing rendered a control that reached it** — the
   * twenty-second time this project has shipped something built and reachable from nothing (pause
   * and resume were #21, and `README.md` keeps the count). The store already clears `selected`
   * when the deleted task was the open one, so the card falls back to the list on its own.
   */
  const removeTask = useTasks((s) => s.remove)

  /*
   * The join with the agents store — see the header. Both selectors return stored values
   * (`check:selectors`' rule); `roles` is derived, so it is memoised on the roster's identity,
   * which `adopt` replaces wholesale on every broadcast. The three run-strip handlers that used
   * to sit beside `roster` moved to `TaskDetailHost` with the card — the strip is the card's.
   */
  const roster = useAgents((s) => s.roster)
  const roles = useMemo(() => rosterRoles(roster), [roster])
  /*
   * The changes the picker offers. (M28) Selected as the board and mapped in a `useMemo`, never
   * in the selector itself: a selector that built a fresh array would re-render for ever and end
   * at *Maximum update depth exceeded* — the rule `check:selectors` enforces.
   */
  const specBoard = useSpec((state) => state.board)
  const changeNames = useMemo(
    () => (specBoard.kind === 'ready' ? specBoard.changes.map((change) => change.name) : []),
    [specBoard],
  )
  /*
   * What the compose dialog's link picker offers: the board's rows, in the panel's own reading
   * order. (M30) `changeNames`' arrangement — derived in a memo, never in a selector.
   */
  const linkTargets = useMemo(
    () =>
      board.kind === 'ready'
        ? linkableTargets(null, board.tasks).map((task) => ({
            id: task.id,
            title: task.title,
            status: task.status,
          }))
        : [],
    [board],
  )
  /* `RunView` is structurally a `RunRef` — the five fields the chip reads, `phase` opaque. */
  const runs: readonly RunRef[] = roster.kind === 'ready' ? roster.runs : NO_RUNS
  const hint = assigneeHint(roster.kind)

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
   * The text the list is narrowed by, `''` for none.
   *
   * Transient gesture state like the filter, and in this window only for the filter's reason: a
   * second cide window on the same project must not find its list narrowed by somebody else's
   * half-typed word.
   *
   * # Unlike the filter, it does NOT survive a project switch
   *
   * The filter's argument for surviving is that a status is not project-scoped — every board
   * has the same four, so the value cannot mean something else after the switch. A query is the
   * opposite: it is a sentence about one project's *content*, `selected`'s situation rather
   * than the filter's, and carried across it silently narrows the new project's board by words
   * typed about the old one. The filtered-to-nothing screen would name the query, so the
   * failure is survivable — but a narrowing the user never asked of *this* project is wrong
   * even when it happens to match something.
   */
  const [query, setQuery] = useState('')
  useEffect(() => setQuery(''), [project])

  /*
   * Which ids the query matched. (M68)
   *
   * The narrowing is `task_search`'s answer, not a local filter, because the rule runs over the
   * **body** and the board stopped carrying bodies — `matchesQuery`'s doc in `model.ts` has the
   * whole argument, including why a local id-and-title filter is the one option not open to us: it
   * would draw *No tasks match "…"* over a task whose description contains exactly what was typed.
   *
   * `null` means *no query*, and it is also the state while an answer is outstanding. That is
   * deliberate — narrowing to nothing for the frame between a keystroke and its answer would flash
   * an empty board — and it is what makes `listEmpty` honest: with `matched` null the "nothing
   * survived" branch cannot be reached, so no sentence is drawn about a search that has not run.
   *
   * Re-run on `rev` as well as on the query: a comment or a new task changes what the same query
   * matches, and a stale id set would go on hiding a row the tracker now has. No debounce — the
   * whole call is one `spawn_blocking` over an in-memory `Vec` and a keystroke costs less than the
   * render it is already causing; a debounce here would buy nothing and make the box feel late.
   */
  const rev = board.kind === 'ready' ? board.rev : null
  const [matched, setMatched] = useState<ReadonlySet<string> | null>(null)
  useEffect(() => {
    if (project === null || query.trim() === '') {
      setMatched(null)
      return
    }
    let live = true
    /*
     * `.catch(notifyFailure)`, never a bare `void`: this is an effect, and an unhandled rejection
     * out of one unmounts the tree under React 19. A refusal leaves `matched` null, which shows
     * everything — the honest failure mode for a narrowing control.
     */
    void tasksApi
      .search(project, query)
      .then((ids) => {
        if (live) setMatched(ids === null ? null : new Set(ids))
      })
      .catch(notifyFailure)
    return () => {
      live = false
    }
  }, [project, query, rev])

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

  /*
   * Disarm whenever the board moves, so a stale arming does not sit in state waiting for a `rev`
   * to come round again. `board` is the store's object identity, and `newerBoard` deliberately
   * hands back the identical object when it drops a snapshot — so an agent's heartbeat that
   * changes nothing disarms nothing, and only a real change does.
   */
  useEffect(() => setDeleteArmed(null), [board])

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
  const guarded = useCallback(
    (done: Promise<void>) => {
      // Stamped with this panel's project, so a write that fails after the user has moved on
      // reports in the project it was aimed at rather than over the next one's board.
      void done.catch((reason: unknown) => notifyFailure(reason, { project }))
    },
    [project],
  )

  return (
    <>
      <TasksPanelView
        project={project}
        board={board}
        runs={runs}
        roles={roles}
        selected={selected}
        filter={filter}
        onFilter={setFilter}
        query={query}
        matched={matched}
        onQuery={setQuery}
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
          roles={roles}
          assigneeHint={hint}
          /*
           * Only on a `ready` board. (M28) `undefined` is what withholds the whole row, so a
           * project with no `openspec/` — or one whose board has not been read yet — sees the
           * dialog exactly as it was before M28.
           */
          changes={specBoard.kind === 'ready' ? changeNames : undefined}
          /*
           * Same structural optionality for links. (M30) Only a `ready` board has rows to link
           * to; withheld otherwise, so the dialog draws exactly as it did before M30.
           */
          tasks={board.kind === 'ready' ? linkTargets : undefined}
          busy={creating}
          onDraft={setDraft}
          onCancel={endCompose}
          onPickAttachments={pickAttachments}
          onStageClipboard={stageClipboard}
          dropHot={dropHot}
          onCreate={(draft) => {
            guarded(
              createTask(draft).then(() => {
                setFilter((current) => filterAfterCreate(current, draft.status))
                // The search's half of the same rule: a query the new row does not match would
                // hide the create exactly the way a wrong filter would. `queryAfterCreate`.
                setQuery((current) => queryAfterCreate(current, draft))
              }),
            )
          }}
        />
      )}
      {/*
        * The card is NOT here — `TaskDetailHost` mounts it, from `App.tsx`, so it opens over
        * whatever panel selected the task. Its `TaskEdit` writes and the live-run strip's
        * handlers moved with it; see both headers.
        */}
    </>
  )
}

/*
 * A module-level constant rather than a fresh literal in the JSX. `TasksPanelView` re-renders on
 * every tick of the clock above, and a new `[]` each time would be a new prop identity for a
 * value that has not changed — harmless today, and exactly the thing that stops being harmless
 * the moment anything downstream memoises on it. This is the non-`ready` fallback only; the
 * ready value is the roster's own array, whose identity `adopt` already manages.
 */
const NO_RUNS: readonly RunRef[] = []
