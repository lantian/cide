/**
 * The Agents panel as the app mounts it: `AgentsPanelView`, plus the stores and the clock. (M18)
 *
 * ```tsx
 * {sidebar.view === 'agents' && <AgentsPanel project={activeProjectId} … />}
 * ```
 *
 * # Why this file exists at all
 *
 * `AgentsPanel.tsx` and `RunRow.tsx` are rendered under node by
 * `ui/scripts/check-agents-render.mjs`, through `react-dom/server`, with nothing stubbed but
 * `window` — which is the only gate in this project that can see a panel that compiles, mounts
 * and draws nothing. That works because those two files read **no store, call no IPC and never
 * read the clock**: every fact and every gesture arrives as a prop. `GitPanelHost` and
 * `TasksPanelHost` exist for the same reason and state it at more length.
 *
 * So this file holds the half a server render cannot follow, and it must stay the only one: a
 * `useAgents` call moved down into the view would drag `@/ipc/client` — and with it
 * `@tauri-apps/api` — into a check whose whole value is that it needs neither.
 *
 * # It reads *both* stores, and that is the cross-link's only structural cost
 *
 * A run row's second line is a button naming the task the run was dispatched against, and the
 * roster carries only the task **id** — `AgentRun::task` is the whole stored half of the
 * cross-link. The titles live in `tasksStore`, so this host joins the two and hands the view a
 * plain `id → title` map. `TasksPanelHost` reaches the other way for the same reason. The views
 * stay ignorant of each other, which is what stops the two panels drifting into two answers for
 * "which agent is on t-14".
 *
 * A missing key is not a failure: `model.ts`'s `titleOf` draws the bare id, which is the honest
 * render when the tracker has not been read (or does not exist) and the run still names a task.
 *
 * # `nowMs` is a prop, and the clock lives here
 *
 * Run rows print elapsed times, so something has to tick. A `Date.now()` inside the view would
 * make its markup differ between two renders of the same fixture and the render check could not
 * digest it at all.
 *
 * One second, and not `TasksPanelHost`'s thirty: `elapsed` has **second** resolution for the
 * first minute — `12s`, then `4m`, then `1h 04m` — so a thirty-second tick would leave a run
 * that has just started reading `0s` for half a minute, on the one row a user watching a
 * dispatch is actually looking at. The interval only exists while this panel is mounted, and
 * the render it causes is over a list bounded by the concurrency ceiling plus `RECENT_CAP`.
 *
 * # All seven run actions are passed now
 *
 * `onDispatch`, `onStop` and `onOpen` were wired first: there is a registry behind them, and
 * `AgentDef.unavailable` is `null` for a sound role, so `canDispatch` returns a green light and
 * the view draws a Dispatch button that has to work.
 *
 * `onPause`, `onResume`, `onRetryTurn` and `onAckStaleTurn` join them, because the four
 * commands behind them exist and are tested. The rule that held them back has not changed and
 * is the reason they can be passed at all: every handler on `AgentsPanelViewProps` is optional
 * precisely so a missing one means the control is **not drawn** rather than drawn dead, so the
 * pause ⏸, the resume ▶ and the stale-turn bar's two buttons appear on the frame these props
 * appear and not before. `RunRow` still gates each on `row.canPause` / `phase === 'paused'` /
 * `row.staleTurn`; nothing here re-derives those, because a second answer to "is there a child
 * to freeze" is how a signal is sent to a run that has finished.
 *
 * # Configure is the one gesture that leaves the panel
 *
 * It opens the project's Settings tab on the agents screen, through `agentsStore.configure`,
 * which also records the role in `focusRole` for the settings screen being written next door to
 * read when it lands. Two props rather than one nullable one — `onConfigure(agent)` from a role
 * row, `onConfigureAll()` from the empty screen's centred button — for the reason `onPause` and
 * `onPauseAll` are two: the gestures differ in what they mean, and a nullable argument would let
 * the screen-level one be passed to a control drawn from a row.
 *
 * # The project-scope pause is here now, and it is the pair the header draws
 *
 * `agents_pause` takes a nullable run, and `null` is the project scope: it shuts the queue and
 * freezes every live run *including the project's own console session*. Which means resume has
 * to be reachable from a control that is not the frozen pane, and the panel header is the place
 * the design names — so `AgentsPanelViewProps` grew `onPauseAll` and `onResumeAll` beside its
 * two per-run props, and `useAgents.pause()` / `.resume()` with no argument are exactly those
 * two gestures. Two lines below, as promised.
 *
 * **The offer condition lives in the view and not here**, and specifically not split across the
 * two. `AgentsPanel.tsx`'s `ScopeControl` decides which of the pair is drawn, from the roster it
 * is already holding; this host passes both handlers whenever a project is open and re-derives
 * nothing. A second answer to "is anything frozen" is how a Resume disappears from a window
 * whose console is frozen, which is the one failure the control exists to prevent.
 *
 * The **palette** carries the same pair — `agents.pause` and `agents.resume` in
 * `cide_core::commands` — and keeps carrying it: the palette is by construction not the frozen
 * pane, and it reaches the scope from a window whose sidebar is showing some other panel
 * entirely, or none.
 *
 * # Which task a dispatch is against, and the sentence when there is none
 *
 * `DispatchRequest.task` is optional on the wire and this panel fills it anyway: a run with no
 * task draws no chip on any row, leaves no trace once it has exited, and puts a hole in the
 * record of what the agents did that afternoon. The Tasks panel's current selection is the
 * answer — read off `useTasks.getState()` rather than subscribed to, because nothing here
 * *draws* the selection and a subscription would re-render the whole panel every time somebody
 * clicked a row in the other one.
 *
 * With nothing selected the dispatch is **refused with a sentence naming the Tasks panel**,
 * rather than sent as an ad-hoc run. Those are the two honest options; the third — silently
 * dispatching against nothing — is the one that produces the unaccountable run this design
 * spends a field avoiding.
 *
 * The intended gesture is smaller than a refusal: an inline row under the role listing the open
 * tasks, plus a one-line instruction, dismissible with Escape. It is not here because it needs
 * a prop `AgentsPanelViewProps` does not have — this slice does not own `AgentsPanel.tsx`. What
 * that view needs is a `dispatchFor?: string | null` (which role's form is open, `null` for
 * none) plus `onDispatchFormOpen?: (agent: string) => void` and
 * `onDispatchSubmit?: (agent: string, task: string, prompt: string) => void`, with `onDispatch`
 * becoming the form-opening gesture rather than the send. Until then, this.
 */
import { memo, useCallback, useEffect, useMemo, useState } from 'react'
import { fsReveal, type ProjectId } from '@/ipc/client'
import { notify, notifyFailure } from '@/chrome/notices'
import { useAgents } from '@/sidebar/agentsStore'
import { useTasks } from '@/sidebar/tasksStore'
import { revealTask } from '@/chrome/taskReveal'
import { AgentsPanelView } from './AgentsPanel'

/** How often elapsed times are recomputed. See the header for why it is not 30 s. */
const TICK_MS = 1_000

/**
 * What a Dispatch press says when no task is selected.
 *
 * It names the panel and the gesture, because "select a task first" is only actionable if the
 * reader knows where tasks are selected — and the Tasks panel may not even be the view on
 * screen when this is read.
 */
const NO_TASK_SELECTED =
  'Select a task in the Tasks panel first. A run is always dispatched against one, so the ' +
  'board can account for what the agent did.'

export interface AgentsPanelProps {
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
export const AgentsPanel = memo(AgentsPanelImpl)

function AgentsPanelImpl({ project }: AgentsPanelProps) {
  const roster = useAgents((s) => s.roster)
  const enable = useAgents((s) => s.enable)
  const dispatch = useAgents((s) => s.dispatch)
  const stop = useAgents((s) => s.stop)
  const pause = useAgents((s) => s.pause)
  const resume = useAgents((s) => s.resume)
  const retryTurn = useAgents((s) => s.retryTurn)
  const ackStaleTurn = useAgents((s) => s.ackStaleTurn)
  const openPane = useAgents((s) => s.openPane)
  const configure = useAgents((s) => s.configure)
  const board = useTasks((s) => s.board)

  const [nowMs, setNowMs] = useState(() => Date.now())
  useEffect(() => {
    setNowMs(Date.now())
    const timer = setInterval(() => setNowMs(Date.now()), TICK_MS)
    return () => clearInterval(timer)
    // `roster` on purpose: a run that has just appeared should read `0s` on the frame it
    // appears rather than inheriting the previous tick's `nowMs`.
  }, [roster])

  /*
   * `id → title`, rebuilt only when the board's identity changes. `tasksStore.adopt` keeps that
   * identity stable across a snapshot it drops, so a broadcast that changes nothing rebuilds
   * nothing — and the map is a new object each time it *is* rebuilt, which is why this is
   * memoised at all: `AgentsPanelView` re-renders every second on the clock above, and a fresh
   * `{}` per tick would be a new prop identity for a value that has not moved.
   */
  const taskTitles = useMemo(() => {
    const titles: Record<string, string> = {}
    if (board.kind === 'ready') for (const task of board.tasks) titles[task.id] = task.title
    return titles
  }, [board])

  /**
   * Every write goes out the same way: fire it, and show the reason if it fails.
   *
   * **Not** wrapped in `pendingCommand`, and the asymmetry with `agents.roster` is the design.
   * This is a user gesture that writes a tracked file into their repository, and a gesture that
   * silently does nothing is the failure this project has paid for most often. `notifyFailure`
   * puts the reason on screen through `chrome/Failures.tsx`; the store deliberately does not
   * import that, so the surfacing stays at the gesture, where a reader looking at the click can
   * find it.
   */
  const guarded = useCallback(
    (done: Promise<void>) => {
      // Stamped with this panel's project, so the reason lands in the project the write was
      // aimed at rather than in whichever one the user has switched to by the time it fails.
      void done.catch((reason: unknown) => notifyFailure(reason, { project }))
    },
    [project],
  )

  /*
   * Which role's Integrate is armed, or `null`.
   *
   * A merge into the branch the user has checked out is the one gesture in this panel that
   * changes their own working tree rather than an agent's, so it is confirm-on-second-click —
   * `TaskDetail`'s delete pattern, and for the same reason: the act is recoverable (the agent's
   * branch still holds its commits, and `worktree::integrate` refuses without touching anything
   * when it would conflict), so a modal is heavier than it is worth, and a bare single click is
   * lighter than it is worth.
   *
   * Host state rather than the store's: it is which button this window is looking at, which is
   * transient gesture state and exactly what the webview is allowed to own. A second window
   * showing the same project should not find its Integrate armed because somebody armed one here.
   */
  const [integrateArmed, setIntegrateArmed] = useState<string | null>(null)

  /*
   * Disarm whenever the roster moves. A button armed against a roster that has since changed is
   * armed against a claim the user made about an older screen — and the roster moves on every
   * dispatch, every turn ending and every `.cide/` write, so "the user armed this and then the
   * world changed" is ordinary rather than rare.
   */
  useEffect(() => setIntegrateArmed(null), [roster])

  /*
   * All three outcomes reach the user, and the refusal is the one that matters.
   *
   * `conflicts` carries the paths, and `notices.detail` is pre-wrapped multi-line text that the
   * toast keeps folded until asked — which is what makes it the right home for a list whose
   * length nobody can predict. Reporting "it conflicted" without them would be a dead end, and
   * `AgentIntegration`'s own doc says every caller is expected to put them in front of the user.
   * `upToDate` is reported too: a button that answers nothing visible is indistinguishable from
   * one that did not fire.
   */
  const integrate = useCallback(
    (agent: string) => {
      setIntegrateArmed(null)
      void useAgents
        .getState()
        .integrate(agent)
        .then((done) => {
          // All three carry the project: an integrate is a merge over a real repository and
          // takes as long as it takes, so its answer routinely arrives after the user has
          // moved on — and a merge report shown over a different repository is worse than one
          // shown late.
          if (done.kind === 'upToDate') {
            notify(`${agent} has nothing to integrate — its branch holds no new commits.`, {
              kind: 'warn',
              project,
            })
            return
          }
          if (done.kind === 'merged') {
            const n = done.files === 1 ? '1 file' : `${done.files} files`
            notify(`Integrated ${agent}: ${n} at ${done.commit.slice(0, 8)}.`, {
              kind: 'ok',
              project,
            })
            return
          }
          notify(`${agent} conflicts with your branch — nothing was merged.`, {
            kind: 'error',
            project,
            hint: 'Your checkout is untouched. Resolve on the agent’s branch, or open its pane and ask it to.',
            detail: done.paths.join('\n'),
          })
        })
        .catch((reason: unknown) => notifyFailure(reason, { project }))
    },
    [project],
  )

  /*
   * `.cide/config.json`'s path, which only the two screens that print it have. `null` on a
   * `ready` roster — there is nothing to reveal from a screen that is not about the file — and
   * on `unknown`, where nobody has looked.
   */
  const configPath =
    roster.kind === 'disabled' || roster.kind === 'empty' ? roster.configPath : null

  return (
    <AgentsPanelView
      project={project}
      roster={roster}
      taskTitles={taskTitles}
      nowMs={nowMs}
      /*
       * The write that touches the user's repository. Passed only with a project open, so the
       * button is absent rather than dead on the "No project open" screen — which never draws
       * it anyway, and that redundancy is deliberate: the rule that a handler's absence
       * withholds the control is enforced here as well as there, so a change to either alone
       * cannot produce one. The three run gestures below take the same guard for the same
       * reason.
       */
      {...(project === null ? {} : { onEnable: () => guarded(enable()) })}
      {...(project !== null && configPath !== null
        ? {
            onRevealConfig: () => {
              /*
               * May legitimately fail on a `disabled` roster: there is nothing to reveal if
               * `.cide/` does not exist yet. The failure is shown rather than swallowed — a
               * Reveal that does nothing looks like a broken button, and this one sits directly
               * beside the sentence promising the user a file is about to appear there.
               */
              void fsReveal.showInManager(project, configPath).catch(notifyFailure)
            },
          }
        : {})}
      {...(project === null
        ? {}
        : {
            /*
             * Dispatch: always against a task, never against nothing. See the header.
             *
             * The refusal is a `notifyFailure` rather than a disabled button, because the
             * button is not what is missing — the role really is dispatchable, and the thing
             * that is absent is a decision the user has not made yet. A control greyed out for
             * a reason that lives in another panel is precisely the state `canDispatch` exists
             * to keep off this screen.
             */
            onDispatch: (agent: string) => {
              const task = useTasks.getState().selected
              if (task === null) {
                notifyFailure(NO_TASK_SELECTED)
                return
              }
              guarded(dispatch(agent, task))
            },
            /*
             * Stop and Open both act on a run id the row was drawn from, and both are gated by
             * the view on top of the handler being present — `canStop` is "not a done phase",
             * `canOpen` is "there is a session to mirror". Neither is re-derived here: a second
             * answer to "is there a transcript to attach to" is how a pane opens on nothing.
             */
            onStop: (run: string) => guarded(stop(run)),
            onOpen: (run: string) => guarded(openPane(run)),
            /*
             * Pause and resume, per run. Gated by the view on `row.canPause` and on the row
             * reading `paused` — one or the other is drawn, never both — so the id that arrives
             * here is one the roster said had a child a paint ago. Not re-checked: the registry
             * refuses a run with no child with a sentence, and a check on this side would be a
             * second, quieter answer that disagrees with it in the one frame that matters.
             *
             * These are the **per-run** scope, and the two below are the project scope. Two
             * pairs of props rather than one pair taking a nullable run, because the gestures
             * differ in what they reach: one signals a worker, the other also freezes the pane
             * the user is typing into.
             */
            onPause: (run: string) => guarded(pause(run)),
            onResume: (run: string) => guarded(resume(run)),
            /*
             * And the **project** scope, which the panel header draws: no argument, which is
             * what `agents_pause`'s nullable run means. It shuts the dispatch queue, freezes
             * every run, and freezes this project's own console session — the pane the user
             * types into — so the resume half must be reachable from a control that is not that
             * pane. The header is that control; `AgentsPanel.tsx`'s `ScopeControl` owns the
             * condition on which it is offered, and nothing here re-derives it.
             *
             * Passed together with the four above rather than gated separately: the same
             * `project !== null` is the whole precondition, and a pause-all wired without its
             * resume-all would be a switch with no way back.
             */
            onPauseAll: () => guarded(pause()),
            onResumeAll: () => guarded(resume()),
            /*
             * The stale-turn bar's two buttons. Two handlers and not one with a flag, all the
             * way down to two `#[tauri::command]`s: *Retry turn* re-sends the prompt and
             * **spends a turn of the user's quota**, *Leave it* spends nothing, and a boolean
             * between a user and a call that costs them money is the wrong shape.
             *
             * Both can be refused — an offer already taken cannot be taken twice — and the
             * refusal goes on screen through `guarded` rather than being swallowed, because a
             * second press of Retry that silently did nothing is indistinguishable from one
             * that silently sent a second prompt.
             */
            onRetryTurn: (run: string) => guarded(retryTurn(run)),
            onAckStaleTurn: (run: string) => guarded(ackStaleTurn(run)),
            /*
             * Configure, and its screen-level twin on the empty panel. The only gesture here
             * that opens a tab rather than touching a run — see `agentsStore.configure`, which
             * also records which role was named for the settings screen that will read it.
             *
             * Passed together with the rest and gated only on a project being open, because the
             * view draws Configure on **every** role row unconditionally: it is the control a
             * user reaches for when the role is broken, so the one state it must not be missing
             * in is the state where nothing else on the row works. `guarded` is what puts a
             * refusal on screen; a Configure that silently did nothing would read as the panel
             * being dead.
             */
            onConfigure: (agent: string) => guarded(configure(agent)),
            onConfigureAll: () => guarded(configure()),
            /*
             * Integrate is a *role* action, not a run action — it merges a branch, which outlives
             * every run that wrote to it. It is offered whatever the role's runs are doing, and
             * `upToDate` is the honest answer when there is nothing on the branch: the panel does
             * not know what a role's worktree holds, and a button that pretended to would be
             * predicting from data it does not have.
             */
            integrateArmed,
            onIntegrateArm: (agent: string) => setIntegrateArmed(agent),
            onIntegrate: integrate,
          })}
      /*
       * The agent→task link: selecting the task is the whole gesture now. `TaskDetailHost` is
       * mounted from `App.tsx` outside the sidebar branches, so the card opens over THIS panel
       * — the sidebar is not switched to Tasks, because a modal that dims the whole window
       * needs no panel under it, and yanking the view out from under the user was ceremony.
       *
       * `TaskId` is a bare `string` in `model.ts` — that file imports nothing — and the newtype
       * is a `string` on the wire too, so this is the seam where the panel's plain id becomes
       * the tracker's id again.
       *
       * The guard and its three refusals live in `chrome/taskReveal.ts` rather than here, and
       * have since a `t-503` in a terminal pane's output became the second road to this same
       * gesture (M60). The refusals are the honest ends of it — `openTask` keeps the card off
       * screen when the board is not `ready` or the id is not on it, so a bare `select` in
       * either state would be a click that visibly does nothing — and two copies of them would
       * have drifted on wording long before anybody noticed they disagreed on the guard.
       * `project` is passed because a task id is unique only within a project; see
       * `taskReveal.ts`.
       */
      onRevealTask={(task: string) => revealTask(task, project)}
      /*
       * Nothing is withheld here any more. Every handler `AgentsPanelViewProps` declares is
       * passed, and each one's control is drawn or not by the view's own gate.
       */
    />
  )
}
