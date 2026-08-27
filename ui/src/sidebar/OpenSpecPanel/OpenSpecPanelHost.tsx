/**
 * The OpenSpec panel's host: everything the view is not allowed to touch. (M28)
 *
 * The store, the IPC, and the transient gesture state — which sections are open — live here, so
 * `OpenSpecPanelView` stays a function of its props and `check:openspec-render` can SSR it under
 * node. `AgentsPanelHost` and `TasksPanelHost` draw the same line for the same reason.
 *
 * The `cide://spec-changed` subscription is deliberately **not** here: it lives in `App.tsx`,
 * because the rail's badge has to stay live while the sidebar is shut or showing Files, and a
 * listener registered inside a panel goes stale the moment that panel unmounts.
 */
import { memo, useCallback, useEffect, useMemo, useState } from 'react'
import { spec as specApi, type ProjectId } from '@/ipc/client'
import { notify, notifyFailure } from '@/chrome/notices'
import { ConfirmDestructive, type ConfirmState } from '@/chrome/ConfirmDestructive'
import { useContextMenu } from '@/menus'
import type { MenuEntry } from '@/menus/model'
import { useSpec } from '../specStore'
import { useTasks } from '../tasksStore'
import { EMPTY_DRAFT } from '../TasksPanel/model'
import { OpenSpecPanelView } from './OpenSpecPanel'
import {
  changeRowActions,
  specRowActions,
  type RowMenuAction,
  type TrackerKind,
} from './model'
import { ConfigDialog } from './ConfigDialog'
import { reloadConsoleAfterSetUp } from './consoleReload'
import { useProposeDialog } from './ProposeDialog'

export interface OpenSpecPanelProps {
  project: ProjectId | null
}

function OpenSpecPanelImpl({ project }: OpenSpecPanelProps) {
  const board = useSpec((state) => state.board)
  const busy = useSpec((state) => state.busy)
  const attach = useSpec((state) => state.attach)
  const refresh = useSpec((state) => state.refresh)
  const setUp = useSpec((state) => state.setUp)

  // Sections start open, so `expanded` holds only what the user has *shut*. A map of what is
  // open would need seeding from the board, and a tree that briefly renders shut on every board
  // arrival is a tree that flickers under an agent.
  const [expanded, setExpanded] = useState<Record<string, boolean>>({})

  /*
   * Which task tracks which change. (M28)
   *
   * The panel listed changes and offered no way to act on one, which made it a viewer of
   * somebody else's directory: the whole lifecycle — approve, dispatch, watch, accept — hangs
   * off a *task* that names a change, and nothing here could reach a task. This is that link.
   *
   * Built in a `useMemo` over the board and never inside the selector: a selector that returned
   * a fresh object would re-render for ever and end at *Maximum update depth exceeded*, which is
   * what `check:selectors` exists to make unrepresentable.
   *
   * First writer wins on a duplicate, and deliberately so — the board is ordered, so two tasks
   * naming one change resolve to the same row on every render rather than flickering between
   * them. A board that fans a change across two tasks has no single answer, and a stable wrong
   * one is easier to notice than an unstable one.
   */
  const taskBoard = useTasks((state) => state.board)
  /*
   * The tracker's own state, passed down beside the map.
   *
   * `tasks` is empty on three of the four arms and the row must not draw them the same way —
   * `rowAction`'s doc has the bug in full: an `unknown` board rendered as *Start work*, and
   * `tasksStore.create` refuses that arm silently, so the button did nothing.
   */
  const tracker: TrackerKind = taskBoard.kind
  const tasks = useMemo(() => {
    const byChange: Record<string, string> = {}
    if (taskBoard.kind !== 'ready') return byChange
    for (const task of taskBoard.tasks) {
      if (task.change !== null && !Object.hasOwn(byChange, task.change)) {
        byChange[task.change] = task.id
      }
    }
    return byChange
  }, [taskBoard])

  /**
   * Start work on a change, or open the task that already tracks it.
   *
   * *Start* creates the task **pre-linked**, which is the ordering `TaskSink::create` argues in
   * Rust: the auto-dispatch trigger reads the task a mutation left behind, so a create-then-link
   * would publish a task with no change and any dispatch from it would be told nothing about the
   * checklist. It deliberately does **not** assign — assigning is what starts an agent, and that
   * is the user's decision to make on the card that has just opened.
   */
  const onRowAction = useCallback(
    (change: string, action: 'start' | 'open') => {
      const store = useTasks.getState()
      if (action === 'open') {
        const existing = tasks[change]
        if (existing !== undefined) store.select(existing as never)
        return
      }
      void store
        .create({ ...EMPTY_DRAFT, title: change, change })
        .then(() => {
          // Selected after the write lands, from the board the create adopted — `create`
          // answers with nothing, and guessing an id here would be inventing one.
          const board = useTasks.getState().board
          /*
           * Both misses below are *silence* if they merely `return`, and silence after a click
           * is the one thing this whole path was reported for. The write may well have landed;
           * what failed is finding the row it made, so the sentence says that rather than
           * claiming the task was not created.
           */
          if (board.kind !== 'ready') {
            notify(`Created a task for ${change}, but the task board has not answered yet.`, {
              kind: 'warn',
            })
            return
          }
          const made = board.tasks.filter((task) => task.change === change).at(-1)
          if (made === undefined) {
            notify(`Created a task for ${change}, but it is not on the board yet.`, {
              kind: 'warn',
            })
            return
          }
          useTasks.getState().select(made.id as never)
        })
        .catch(notifyFailure)
    },
    [tasks],
  )

  useEffect(() => {
    attach(project)
    /*
     * And a read on **every mount**, not only when the project changes.
     *
     * This panel is conditionally rendered, so a mount *is* the user opening it — which makes
     * this the one moment a re-read is certainly wanted and certainly affordable. It is also the
     * recovery path for a board that went stale: the watcher can miss a burst (an agent writing
     * a whole change tree faster than a directory watch can be taken), and without this the only
     * way back to the truth was relaunching cide. `attach` is a no-op when the project has not
     * changed, so the two together are exactly one read.
     */
    if (project !== null) void refresh().catch(notifyFailure)
    // Deliberately mount-only: `refresh` is stable and re-running this on every render would put
    // two subprocesses behind every keystroke that touches this component.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [attach, project])

  const onToggleSection = useCallback((id: string) => {
    setExpanded((current) => ({ ...current, [id]: current[id] === false }))
  }, [])


  /*
   * Both rows open a **page**, not a file. (M28)
   *
   * They opened `specs/<id>/spec.md` and `changes/<name>/proposal.md`, which answered *show me
   * this* with one document out of five and no way to reach the rest — the checklist, the
   * requirement edits, whether it validates, or anything that could be done about any of it.
   * `TabKind::OpenSpec` carries the argument; the files are still one click away from the page.
   */
  const onOpenSpec = useCallback(
    (id: string) => {
      if (project === null) return
      void specApi.openTab(project, { kind: 'capability', spec: id as never })
    },
    [project],
  )
  const onOpenChange = useCallback(
    (name: string) => {
      if (project === null) return
      void specApi.openTab(project, { kind: 'change', change: name as never })
    },
    [project],
  )

  const onSetUp = useCallback(() => {
    void setUp()
      .then(() => {
        /*
         * Fix the one thing that is about to go wrong, at the moment it becomes true.
         *
         * `openspec init --tools claude` writes `.claude/skills/openspec-<name>/SKILL.md`, and Claude
         * Code reads a project's skills **once, at startup** — so the conversation already open
         * in this project does not have them, and the very next press of *Propose* would be
         * refused by `spec_run_command` with a sentence about restarting the pane. This shipped
         * as that sentence shown here as a notice, and the report on it asked the right
         * question: cide owns the pane's respawn, so cide restarts it. `consoleReload.ts` is
         * the mechanism and the argument — which pane, why resume, and why the wizard's door
         * goes through the same road.
         */
        return reloadConsoleAfterSetUp(project)
      })
      .catch(notifyFailure)
  }, [setUp, project])

  const onRetry = useCallback(() => {
    void refresh().catch(notifyFailure)
  }, [refresh])

  /*
   * The composer lives in `ProposeDialog.tsx` and is rendered by `App.tsx`. (M28)
   *
   * It was panel state — a box that grew under the toolbar — and there are three doors to it now
   * (these two buttons and the `spec.propose` / `spec.explore` palette commands), two of which
   * exist whether or not this panel is mounted. So the open/shut fact is a small store, and the
   * only thing left here is telling it which command a button means.
   */
  const asking = useProposeDialog((state) => state.command)
  const openCompose = useProposeDialog((state) => state.open)
  const closeCompose = useProposeDialog((state) => state.close)

  const onAsk = useCallback(
    (command: string | null) => {
      if (command === null) closeCompose()
      else openCompose(command)
    },
    [openCompose, closeCompose],
  )

  /*
   * The configuration modal. (M28)
   *
   * Open/shut only — the dialog owns its own reads, its own draft and its own Save, and it is
   * keyed on the project so switching projects rebuilds it rather than leaving one project's
   * unsaved draft over another project's file.
   *
   * It replaces `SettingsSection::OpenSpec`. cide's Settings is a *global* surface and
   * `openspec/config.yaml` is a committed file in one repository; a section in that list said it
   * was a preference. See `ConfigDialog`'s header for the argument.
   */
  const [configuring, setConfiguring] = useState(false)
  // Shut whenever there is no project to configure: a modal left standing across a project close
  // would be a form over a path that no longer resolves.
  useEffect(() => {
    if (project === null) setConfiguring(false)
  }, [project])

  /*
   * Withheld while nobody has looked, and *only* then.
   *
   * `unknown` is "we have not asked yet", which is a different screen from every other arm — a
   * gear there would offer to configure a project whose state cide cannot describe. Every other
   * arm draws it, `absent` included: on that arm the dialog is the init wizard.
   */
  const onConfigure = useCallback(() => setConfiguring(true), [])

  /*
   * The row menu. (M28)
   *
   * Built at the moment of the right-click, from the element under the pointer — so nothing here
   * is computed until the gesture happens and nothing can be stale, which is `useContextMenu`'s
   * own rule for `items`.
   */
  const [confirming, setConfirming] = useState<ConfirmState | null>(null)

  /** Run one menu action against one change. */
  const runChange = useCallback(
    (change: string, action: RowMenuAction['id']) => {
      if (project === null) return
      if (action === 'open') {
        onOpenChange(change)
        return
      }
      if (action === 'validate') {
        void specApi
          .validate(project, change as never)
          .then((verdict) => {
            const blocking = verdict.issues.filter(
              (issue) => issue.level.toLowerCase() === 'error',
            )
            // Both answers are reported. A validate that said nothing when a change was clean
            // would be indistinguishable from one that did not run.
            if (verdict.valid && blocking.length === 0) {
              notify(`${change} validates.`, { kind: 'ok' })
              return
            }
            notify(
              blocking.length === 1
                ? `${change}: 1 problem.`
                : `${change}: ${blocking.length} problems.`,
              { kind: 'error', detail: blocking.map((issue) => issue.message).join('\n') },
            )
          })
          .catch(notifyFailure)
        return
      }

      /*
       * Archive: plan first, then confirm, then run.
       *
       * The plan is a subprocess — it validates and reads the deltas — so it cannot be part of
       * building the menu. A refusal therefore arrives *after* the click, and is shown as a
       * notice naming what to do rather than as a dialog the user then has to dismiss for
       * nothing.
       */
      void specApi
        .changePlan(project, change as never)
        .then((plan) => {
          if (plan.refusals.length > 0) {
            notify(`${change} cannot be archived yet.`, {
              kind: 'error',
              detail: plan.refusals.join('\n\n'),
            })
            return
          }
          setConfirming({
            title: `Archive ${change}?`,
            body:
              'Merges this change’s requirement edits into openspec/specs/ — the project’s ' +
              'source of truth — and moves the change to openspec/changes/archive/. Both are ' +
              'ordinary file edits in your repository, so git is the way back.',
            // Named, not counted: `ConfirmDestructive`'s rule is that the user is about to act
            // on *specific* files and "3 requirements" is not something anybody can check.
            files: plan.specsTouched.map(
              (touch) =>
                `openspec/specs/${touch.spec}/spec.md — ${touch.requirements} ${
                  touch.requirements === 1 ? 'requirement' : 'requirements'
                } ${touch.operation}`,
            ),
            confirmLabel: 'Archive',
            mark: 'file-diff',
            run: () => {
              void specApi
                .archiveChange(project, change as never)
                .then((outcome) => {
                  if (outcome.kind === 'refused') {
                    notify(`${change} was not archived.`, {
                      kind: 'error',
                      detail: outcome.plan.refusals.join('\n\n'),
                    })
                    return
                  }
                  notify(`${change} archived.`, { kind: 'ok' })
                })
                .catch(notifyFailure)
            },
          })
        })
        .catch(notifyFailure)
    },
    [onOpenChange, project],
  )

  const { onContextMenu, menu } = useContextMenu({
    label: 'OpenSpec',
    items: ({ target }) => {
      const changeRow = target?.closest<HTMLElement>('[data-change]')
      const specRow = target?.closest<HTMLElement>('[data-spec]')

      if (changeRow != null) {
        const name = changeRow.dataset['change']
        const row =
          board.kind === 'ready'
            ? board.changes.find((candidate) => candidate.name === name)
            : undefined
        if (name === undefined || row === undefined) return []
        return toEntries(
          changeRowActions({ done: row.completed, total: row.total }),
          (action) => runChange(name, action),
        )
      }

      if (specRow != null) {
        const id = specRow.dataset['spec']
        if (id === undefined) return []
        return toEntries(specRowActions(), () => onOpenSpec(id))
      }

      // Empty space below the last row opens nothing — an empty box at the pointer reads as a
      // broken surface rather than as one with nothing to offer. `ChangesTree`'s rule.
      return []
    },
  })

  return (
    <>
    <OpenSpecPanelView
      board={board}
      expanded={expanded}
      busy={busy}
      onToggleSection={onToggleSection}
      onOpenChange={onOpenChange}
      tasks={tasks}
      tracker={tracker}
      onRowAction={onRowAction}
      onOpenSpec={onOpenSpec}
      onSetUp={onSetUp}
      asking={asking}
      onAsk={onAsk}
      onRetry={onRetry}
      onConfigure={project === null || board.kind === 'unknown' ? undefined : onConfigure}
      onContextMenu={onContextMenu}
      menu={menu}
    />
    {confirming !== null && (
      <ConfirmDestructive
        state={confirming}
        onCancel={() => setConfirming(null)}
        onConfirm={() => {
          const run = confirming.run
          setConfirming(null)
          run?.()
        }}
      />
    )}
    {configuring && project !== null && (
      <ConfigDialog project={project} onClose={() => setConfiguring(false)} />
    )}
    </>
  )
}

/**
 * A row's actions as menu entries.
 *
 * `disabledReason` carries straight through, which is the whole reason the model returns one
 * rather than a boolean: `MenuItem` has no `disabled`, deliberately — a greyed row with no
 * explanation is the single most common way this app has wasted somebody's time, and requiring
 * the sentence to disable the item makes the explanation unskippable.
 */
function toEntries(
  actions: readonly RowMenuAction[],
  run: (action: RowMenuAction['id']) => void,
): MenuEntry[] {
  return actions.map((action) => ({
    id: action.id,
    label: action.label,
    ...(action.danger === true ? { danger: true } : {}),
    ...(action.disabledReason === undefined
      ? { run: () => run(action.id) }
      : { disabledReason: action.disabledReason }),
  }))
}

export const OpenSpecPanel = memo(OpenSpecPanelImpl)
