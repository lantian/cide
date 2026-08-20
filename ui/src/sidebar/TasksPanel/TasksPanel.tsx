/**
 * The ☰ Tasks sidebar panel: `.cide/tasks.json`, grouped, with the task→agent cross-link. (M18)
 *
 * # This panel does not depend on subagents being enabled
 *
 * Stated here as well as in `model.ts` so nobody "fixes" it. A shared, committed task list is
 * useful on its own — it is a tracker in the repository, readable in a pull request, with
 * `git log -p .cide/tasks.json` as its history — and orchestration is **off by default**. Gating
 * this panel on it would make the first thing a curious user clicks say "turn on a feature you
 * have not read about". Nothing here reads a roster; the one place the two meet is `agentChip`,
 * and with no runs it degrades to "the assigned role, dim", which is a correct rendering rather
 * than a stub.
 *
 * # A pure view
 *
 * No store, no IPC, no clock, and — since the card moved into a modal — nothing that needs one:
 * every fact and every gesture arrives as a prop, so `check-agents-render.mjs` can render this
 * through `react-dom/server` with nothing stubbed but `window`. `TasksPanelHost` holds the half
 * a server render cannot follow, and `adapt.ts` is the seam that owns the wire types.
 *
 * # Four board states, and the fourth is the one that is easy to drop
 *
 * `unknown` — *nobody has looked yet* — draws **nothing**. `tasks.board` reaches the store
 * through `pendingCommand` with a `null` fallback, so there is a real interval on every open of
 * this panel in which the answer is not in. Drawing `absent`'s screen during it would say
 * *"there is no tracker in this project"* one frame before the tracker arrives, under a button
 * that creates a file. `ui/scripts/check-agents-render.mjs` pins it from the other end: the
 * `board-unknown` story renders no button and none of the designed sentences.
 *
 * `unreadable` offers **only** Reveal and Retry, and `canWrite(board)` is what enforces it in
 * one place. An app that "recovers" from an unparseable tracker by overwriting it has destroyed
 * the user's data in order to fix its own display — and the likeliest cause of an unparseable
 * `.cide/tasks.json` is a half-resolved merge conflict, which is to say a file that still
 * contains both sides of everything.
 *
 * `absent` offers one `New task` button, and the path is printed in full **above** it, because a
 * control that quietly adds a tracked file to somebody's repository is a surprise commit. The
 * button opens `TaskComposeModal` and writes nothing itself; the file is created by the Create
 * inside it, along with the first task. (M21) That ordering is why the path belongs on this
 * screen rather than in the dialog: it is the sentence that has to be readable before the click,
 * and the dialog is already past it.
 *
 * # The open task's card is not here
 *
 * It used to be: `selected` replaced this whole body with `TaskDetail`. It is now a modal —
 * `TaskDetailModal`, mounted by `TasksPanelHost` **beside** this view rather than instead of it,
 * so the board stays on screen behind the scrim and the filter row does not vanish for as long
 * as a task is open. What is left here is one line of it: the open task's row is marked, so the
 * list behind the scrim says which of its rows the card belongs to.
 *
 * The card lives in its own component because `OverlayCard` portals to `document.body`, and a
 * portal cannot be server-rendered at all. Rendering it from here would take this view — every
 * board state, every group, the filter, both empty screens — out of `check-agents-render.mjs`
 * with it. See `TaskDetail.tsx`'s header.
 */
import {
  BOARD_UNKNOWN,
  GROUP_ORDER,
  agentChip,
  armedDelete,
  canWrite,
  groups,
  listEmpty,
  metaFigure,
  statusGlyph,
  statusLabel,
  statusTone,
  type ArmedDelete,
  type Board,
  type RunRef,
  type StatusFilter,
  type TaskView,
} from './model'
import { phaseGlyph, type RunPhase } from '@/sidebar/AgentsPanel/model'
import { DeleteControl, TONE_CLASS, chipClass, cx } from './TaskDetail'
import type { ProjectId } from '@/ipc/client'
import styles from './TasksPanel.module.css'

export interface TasksPanelViewProps {
  /** The active project, or `null` before one is open. */
  project: ProjectId | null
  /**
   * The tracker.
   *
   * Defaults to [`BOARD_UNKNOWN`] — *nobody has looked* — and never to `absent`, because a
   * default that makes a claim is a claim made by omission.
   */
  board?: Board | undefined
  /**
   * Every run cide knows about, for the task→agent chip.
   *
   * Empty is the ordinary case and is not a degraded one: with no runs a task with an assignee
   * draws a dim chip and a task without draws nothing, which is exactly right.
   */
  runs?: readonly RunRef[] | undefined
  /** Agent id → label. */
  roles?: Readonly<Record<string, string>> | undefined
  /**
   * Which task's card is open, or `null` for none.
   *
   * The card itself is `TaskDetailModal`, mounted beside this view. What this prop does *here*
   * is mark the row it belongs to: the list is still on screen behind the scrim, and a modal
   * over a list of four near-identical rows that does not say which one it came from makes the
   * user close it to find out. A prop rather than state, so this component stays a function of
   * its props and the selection survives the panel unmounting.
   */
  selected?: string | null | undefined
  /** Open a task's card, or (with `null`) close it. */
  onSelectTask?: ((task: string | null) => void) | undefined
  /** Create a task — and, on an `absent` board, the tracker file with it. */
  onCreateTask?: (() => void) | undefined
  /** Show `.cide/tasks.json` in the file manager. The only write-free action on a broken file. */
  onReveal?: (() => void) | undefined
  /** Read the file again. The other one. */
  onRetry?: (() => void) | undefined
  /**
   * Which status the list is narrowed to, or `null` for all of them.
   *
   * A prop, and the state is `TasksPanelHost`'s, for the reason `selected` is: this component
   * stays a function of its props so `check-agents-render.mjs` can render it under node. See the
   * host for the two decisions — that the filter survives a project switch, and that it does not
   * reach the header figure.
   */
  filter?: StatusFilter | undefined
  /** Narrow the list, or (with `null`) widen it back to the whole board. */
  onFilter?: ((filter: StatusFilter) => void) | undefined
  /**
   * The delete the user has armed — **which task, and on which board**.
   *
   * Passed as the whole `ArmedDelete` rather than an id because `armedDelete` is what decides
   * whether it still applies, and it needs the `rev` to do it. Resolving it here, once, is what
   * stops the list row and the card from answering "is this armed" differently.
   */
  deleteArmed?: ArmedDelete | null | undefined
  /** First click: arm a delete. Writes nothing. */
  onDeleteArm?: ((task: string) => void) | undefined
  /** Second click: remove the task from `.cide/tasks.json`. Reachable only from an armed control. */
  onDelete?: ((task: string) => void) | undefined
}

export function TasksPanelView({
  project,
  board = BOARD_UNKNOWN,
  runs = [],
  roles = {},
  selected = null,
  filter = null,
  onFilter,
  deleteArmed = null,
  onDeleteArm,
  onDelete,
  onSelectTask,
  onCreateTask,
  onReveal,
  onRetry,
}: TasksPanelViewProps) {
  /*
   * The one gate every writing control goes through, asked once. Two copies of "may this panel
   * offer a write" is how an `unreadable` board grows a New task button in a later edit.
   */
  const writable = canWrite(board)
  const list = groups(board, filter)
  /*
   * Why the list is empty, when it is — and the two answers are different screens. `'tracker'`
   * is the one that already existed; `'filter'` is a board with tasks in it, none of which match,
   * and drawing nothing for that is how a user concludes their tasks are gone. `null` when there
   * is something to draw, and for every board arm that has its own designed screen.
   */
  const empty = listEmpty(board, filter)
  /*
   * Resolved **once**, here, and handed down as a boolean. `armedDelete` refuses an arming whose
   * board has moved on, and doing that in one place is what stops the row and the card from
   * disagreeing about whether the user's second click still applies.
   */
  const armed = armedDelete(board, deleteArmed)

  const newTask =
    writable && onCreateTask !== undefined ? (
      <button
        type="button"
        className={cx(styles.action, styles.actionPrimary)}
        data-audit="tasksNew"
        data-write="true"
        onClick={onCreateTask}
      >
        New task
      </button>
    ) : null

  /*
   * The filter row: five toggles, `All` and the four statuses in `GROUP_ORDER`.
   *
   * **A row of toggles rather than a `<select>`**, and the width is what settles it. Four
   * statuses is few enough to draw them all, and drawn all at once the control answers two
   * questions at a glance — what the vocabulary is, and which one is on — where a `<select>`
   * answers the second only and hides the first behind a click. It is also one click to change
   * rather than three (open, move, choose) on a control the user pokes repeatedly.
   *
   * `GROUP_ORDER` and not `TASK_STATUSES`, so the buttons sit in the same order as the headings
   * directly beneath them; a filter row whose order disagrees with the list it filters makes the
   * eye do a translation on every use. (`TaskDetail`'s status segment is `TASK_STATUSES` for the
   * opposite reason — it is a progression a task moves *along*.)
   *
   * Drawn only for a `ready` board that has tasks in it: there is nothing to narrow on the other
   * screens, and an empty tracker with a filter row on it offers a control that cannot change
   * anything. It **is** drawn when the filter matches nothing, which is the case that matters —
   * a filter that hid its own control would leave the user with no way back to their tasks.
   */
  const filterRow =
    board.kind === 'ready' && board.tasks.length > 0 && onFilter !== undefined ? (
      <div
        className={styles.filters}
        data-audit="tasksFilters"
        role="group"
        aria-label="Filter tasks by status"
      >
        {[null, ...GROUP_ORDER].map((status) => {
          const on = filter === status
          return (
            <button
              key={status ?? 'all'}
              type="button"
              className={cx(styles.filter, on ? styles.filterOn : undefined)}
              data-audit="tasksFilter"
              data-status={status ?? ''}
              data-on={on ? 'true' : 'false'}
              /* `aria-pressed` rather than `aria-selected`: these are toggle buttons in a group,
                 not tabs, and nothing below them is a tabpanel. */
              aria-pressed={on}
              onClick={() => onFilter(status)}
            >
              {status === null ? 'All' : statusLabel(status)}
            </button>
          )
        })}
      </div>
    ) : null

  return (
    <aside className={styles.panel} data-audit="sidebarTasks" aria-label="Tasks">
      <div className={styles.header} data-audit="tasksHeader">
        <span className={styles.headerTitle}>Tasks</span>
        {/* Empty, not `—`, when `metaFigure` withholds it — the model argues that out. What
            matters is that it is never `0`, which would be a count nobody took. Withheld
            outright with no project open: a figure counting tasks in a project that is not on
            screen is a number about nothing. */}
        <span className={styles.headerMeta} data-audit="tasksMeta">
          {project === null ? '' : (metaFigure(board) ?? '')}
        </span>
      </div>

      {project === null ? (
        <div className={styles.body} data-audit="tasksBody">
          <p className={styles.claim}>No project open</p>
          <p className={styles.detail}>
            The task tracker lives in the project, at <code>.cide/tasks.json</code>. Open one
            from the header’s <b>+</b> button.
          </p>
        </div>
      ) : board.kind === 'unknown' ? (
        /* Nobody has looked. Nothing is drawn — see the file header. */
        <div className={styles.body} data-audit="tasksBody" />
      ) : board.kind === 'absent' ? (
        <div className={styles.body} data-audit="tasksBody">
          <p className={cx(styles.claim, styles.claimQuiet)} data-audit="tasksClaim">
            No task tracker in this project.
          </p>
          <p className={styles.detail}>
            {board.hint.trim() !== ''
              ? board.hint
              : 'Tasks are how the session in your console tab hands work to a subagent and reads back what happened.'}
          </p>
          <p className={styles.warn} data-audit="tasksWarn">
            Creating the first task writes this file, which your repository will contain:
            <code className={styles.path} data-audit="tasksPath">
              {board.path}
            </code>
          </p>
          <div className={styles.actions} data-audit="tasksActions">
            {newTask}
            {onReveal !== undefined && (
              <button
                type="button"
                className={styles.action}
                data-audit="tasksReveal"
                onClick={onReveal}
              >
                Reveal .cide/
              </button>
            )}
          </div>
        </div>
      ) : board.kind === 'unreadable' ? (
        <div className={styles.body} data-audit="tasksBody">
          <p className={cx(styles.claim, styles.claimError)} data-audit="tasksClaim">
            This project’s task file could not be read.
          </p>
          {/*
            * Two controls, and deliberately no third.
            *
            * No New task, no Reset, no repair. The panel is looking at a file it does not
            * understand, in a repository, that somebody else may be mid-merge on; the only
            * honest moves are to show it to a human and to look again.
            */}
          <p className={styles.detail}>
            Nothing here will write to it. Open it and fix it by hand — most often this is a
            merge conflict that was never resolved.
          </p>
          <code className={styles.path} data-audit="tasksPath">
            {board.path}
          </code>
          <pre className={styles.error} data-audit="tasksError">
            {board.error}
          </pre>
          <div className={styles.actions} data-audit="tasksActions">
            {onReveal !== undefined && (
              <button
                type="button"
                className={styles.action}
                data-audit="tasksReveal"
                onClick={onReveal}
              >
                Reveal file
              </button>
            )}
            {onRetry !== undefined && (
              <button
                type="button"
                className={styles.action}
                data-audit="tasksRetry"
                onClick={onRetry}
              >
                Retry
              </button>
            )}
          </div>
        </div>
      ) : empty === 'tracker' ? (
        /*
         * The tracker itself is empty — read, and holding nothing. Note the condition: it is
         * `listEmpty`'s answer and **not** `list.length === 0`, which is the same thing only
         * while there is no filter. Left as it was, a filter matching nothing would print
         * *"No tasks yet"* over a tracker with four tasks in it.
         */
        <div className={styles.body} data-audit="tasksBody">
          <p className={styles.claim} data-audit="tasksClaim">
            No tasks yet.
          </p>
          <p className={styles.detail}>
            A task is a title, a body, a status and the role it is for. Agents read and write
            them through the same file you do.
          </p>
          <div className={styles.actions} data-audit="tasksActions">{newTask}</div>
        </div>
      ) : empty === 'filter' ? (
        /*
         * A board with tasks in it, and none of them in this group.
         *
         * **A different screen from the one above, deliberately.** The two states look identical
         * from inside the list — nothing to draw — and they are opposite facts about the user's
         * project. A filtered board that silently drew nothing, or worse drew *"No tasks yet"*,
         * is exactly how somebody concludes their tasks are gone; so the sentence names the
         * filter, the count says how many tasks the tracker actually holds, and the way out is
         * on screen twice — the filter row, which is deliberately still drawn, and a Show all
         * button in the sentence's own actions.
         */
        <div className={styles.body} data-audit="tasksBody">
          <div className={styles.actions} data-audit="tasksActions">{newTask}</div>
          {filterRow}
          <div className={styles.noMatch} data-audit="tasksNoMatch">
            <p className={cx(styles.claim, styles.claimQuiet)} data-audit="tasksClaim">
              {`No tasks in ${filter === null ? 'this view' : statusLabel(filter)}.`}
            </p>
            <p className={styles.detail}>
              {board.kind === 'ready' && board.tasks.length === 1
                ? 'This tracker holds 1 task, in another group.'
                : `This tracker holds ${board.kind === 'ready' ? board.tasks.length : 0} tasks, all in other groups.`}{' '}
              Nothing has been deleted — the filter above is narrowing the list.
            </p>
            <div className={styles.actions} data-audit="tasksActions">
              {onFilter !== undefined && (
                <button
                  type="button"
                  className={styles.action}
                  data-audit="tasksShowAll"
                  onClick={() => onFilter(null)}
                >
                  Show all tasks
                </button>
              )}
            </div>
          </div>
        </div>
      ) : (
        <div className={styles.body} data-audit="tasksBody">
          <div className={styles.actions} data-audit="tasksActions">{newTask}</div>
          {filterRow}
          <div className={styles.groups} data-audit="tasksGroups">
            {list.map((group) => {
              const rows = (
                <div className={styles.rows} data-audit="tasksRows">
                  {group.tasks.map((task) => (
                    <TaskLine
                      key={task.id}
                      task={task}
                      runs={runs}
                      roles={roles}
                      onSelect={onSelectTask}
                      /* The card is a modal over this list, so the row it came from has to be
                         findable behind the scrim. */
                      open={selected === task.id}
                      armed={armed === task.id}
                      /* Gated on `writable` here rather than inside the row, so the "may this
                         panel offer a write" question stays asked in one place. An `unreadable`
                         board draws no rows at all, but the gate is what keeps that true when a
                         fifth board arm arrives. */
                      onDeleteArm={writable ? onDeleteArm : undefined}
                      onDelete={writable ? onDelete : undefined}
                    />
                  ))}
                </div>
              )
              /* Done is collapsed: it grows without bound and is read least. A `<details>`,
                 so the collapse costs no state and this component stays pure. */
              return group.status === 'done' ? (
                <details className={styles.doneGroup} data-audit="tasksGroup" key={group.status}>
                  <summary className={styles.doneSummary}>
                    <span className={styles.groupTitle}>{group.label}</span>
                    <span className={styles.groupCount}>{group.tasks.length}</span>
                  </summary>
                  {rows}
                </details>
              ) : (
                <div className={styles.group} data-audit="tasksGroup" key={group.status}>
                  <div className={styles.groupHead}>
                    <span className={styles.groupTitle}>{group.label}</span>
                    <span className={styles.groupCount}>{group.tasks.length}</span>
                  </div>
                  {rows}
                </div>
              )
            })}
          </div>
        </div>
      )}
    </aside>
  )
}

/**
 * One task row: `▸ t-14  Add the retry bar        ◍ Developer  ×`.
 *
 * The chip on the right is the whole cross-link, and the list row and the detail strip make the
 * same `agentChip` call so they cannot disagree about which of its three states a task is in.
 * A live run is lit with its phase dot; an assignment with nothing running is dim and dotless;
 * neither draws nothing at all.
 *
 * # The row is a `<div>` wrapping a button, and it has to be
 *
 * It used to *be* the button. It cannot be any more, because a delete control on the row would
 * then be a `<button>` inside a `<button>` — which is invalid HTML, which browsers reconcile by
 * closing the outer element early, and which would therefore put the delete control *outside*
 * the row it belongs to. So the opening half is `.rowMain` and the row is the flex container
 * around it and the delete control. `check-agents-render.mjs`'s `buttons` digest reads
 * `<button…>…</button>` non-greedily and would have digested the nesting as garbage, which is a
 * second, smaller reason to keep them siblings.
 *
 * # Why the delete lives on the row at all
 *
 * `TaskDetail` has one too, and that is not a duplicate: the sidebar is where the whole board
 * is visible, and a task created by mistake wants deleting without a detour through opening it.
 * The card is where you delete the one you are already reading. Both go through `DeleteControl`,
 * so there is one answer to what confirms a delete.
 */
function TaskLine({
  task,
  runs,
  roles,
  onSelect,
  open,
  armed,
  onDeleteArm,
  onDelete,
}: {
  task: TaskView
  runs: readonly RunRef[]
  roles: Readonly<Record<string, string>>
  onSelect?: ((task: string | null) => void) | undefined
  /** Is this the row whose card is open? See the prop's doc on `selected`. */
  open: boolean
  /** Is this row's delete waiting for its confirming second click? `armedDelete`'s answer. */
  armed: boolean
  onDeleteArm?: ((task: string) => void) | undefined
  onDelete?: ((task: string) => void) | undefined
}) {
  const chip = agentChip(task, runs, roles)
  const body = (
    <>
      <span
        className={cx(styles.glyph, TONE_CLASS[statusTone(task.status)])}
        data-audit="tasksGlyph"
        data-status={task.status}
        aria-hidden="true"
      >
        {statusGlyph(task.status)}
      </span>
      <span className={styles.taskId}>{task.id}</span>
      <span className={styles.taskTitle}>{task.title}</span>
      {chip !== null && (
        <span
          className={chipClass(chip)}
          data-audit="tasksChip"
          data-lit={chip.lit ? 'true' : 'false'}
          data-tone={chip.tone}
          title={chip.lit ? `${chip.label} is working on this now` : `Assigned to ${chip.label}`}
        >
          {/* The dot only when a run is actually behind the chip. `phaseGlyph` is
              `AgentsPanel/model.ts`'s, imported rather than restated: a second glyph table
              would be a second answer to what a phase looks like, and the two would drift on
              the first phase either of them learned. The cast is deliberate — the chip carries
              the phase verbatim and unvalidated, which is what that function's own guard is
              for. */}
          {chip.lit && (
            <span className={styles.chipDot} aria-hidden="true">
              {phaseGlyph((chip.phase ?? '') as RunPhase)}
            </span>
          )}
          <span className={styles.chipLabel}>{chip.label}</span>
        </span>
      )}
    </>
  )
  /* Static text rather than a dead button when no host can open a task — `ProblemsPanel`'s
     `rowTag` rule, and the same argument: a row that looks clickable and is not is worse than
     one that does not pretend. */
  return (
    <div
      className={cx(styles.row, open ? styles.rowOpen : undefined)}
      data-audit="tasksRow"
      data-task={task.id}
      data-open={open ? 'true' : 'false'}
    >
      {onSelect === undefined ? (
        <span className={styles.rowMain}>{body}</span>
      ) : (
        <button
          type="button"
          className={styles.rowMain}
          data-audit="tasksOpenTask"
          onClick={() => onSelect(task.id)}
        >
          {body}
        </button>
      )}
      <DeleteControl
        task={task.id}
        label={task.title.trim() !== '' ? task.title : task.id}
        armed={armed}
        compact
        onDeleteArm={onDeleteArm}
        onDelete={onDelete}
      />
    </div>
  )
}
