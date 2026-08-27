/**
 * Getting back to the conversation a task's work was handed to. (M28)
 *
 * `Task::session` is a **record of where the work went** and deliberately survives the pane being
 * closed — the Rust field's own doc says so. That is the right shape and it left a hole in the
 * UI: closing the Claude pane that was working a task left the card naming a conversation with
 * nothing to press. The only road forward was to dispatch the work somewhere else, which throws
 * away everything the conversation had already worked out.
 *
 * # Two gestures, and the difference is a nudge
 *
 * A `SessionId` **is** the value cide passes to `claude --session-id`, which is what makes resume
 * free: `claude --resume <id>` brings the whole transcript back under the same id, so every record
 * that names it — the task's `session` field above all — goes on naming the right conversation.
 * That single spawn is what both buttons are built on.
 *
 * * **Open** puts the conversation back on screen and stops. The CLI restores the transcript and
 *   waits for input; nothing is asked of the model.
 * * **Resume** does that and then types the task in, through the *same* `spec_dispatch_to_session`
 *   every other dispatch uses — so what the model is asked is the sentence every dispatched run
 *   gets, built by `cmd::agents::opening_prompt`, and not a second wording invented here that
 *   would drift from it.
 *
 * A conversation that is *already* on screen needs no spawn for either: Open reveals it, and
 * Resume reveals it and types.
 *
 * # Why this is a frontend gesture and not a command
 *
 * `AgentsPanel/openRun.ts` carries the argument in full and it applies here unchanged: a pane
 * built **in the domain** arrives in the webview over `cide://workspace-changed`, so it never
 * passes through `rememberSpawnPlan`/`takeSpawnPlan`, so `TerminalPane` never sees the intent —
 * and for a mirror that meant closing the pane killed somebody else's child. The split machinery
 * is where a pane's provenance is recorded, so the gesture belongs on this side of the wire.
 *
 * The ownership flag is the one thing this gesture must get the *opposite* way round from
 * `openRun`: a resumed pane **spawned** its child, so it owns it and closing the pane must end
 * it. `TerminalPane`'s `resume` branch therefore leaves `mirrored` unset, which is why `Resume`
 * could not have been folded into `SplitIntent::Mirror`.
 *
 * # Not exported through `index.ts`
 *
 * `ui/scripts/check-agents.mjs` compiles `model.ts` alone with a bare `tsc` and no tsconfig. This
 * module reaches the store and the client, so a barrel entry would put the whole app graph one
 * hop from a check whose entire value is that it needs none of it.
 */
import { revealPane } from '@/editor/revealPane'
import { activeProjectOf, consolePaneOf } from '@/keys/target'
import { paneSessionId } from '@/layout/paneHosts'
import { useWorkspace } from '@/store/workspace'
import { session as sessionApi, spec as specApi, type Pane, type PaneId, type Project, type ProjectId } from '@/ipc/client'

/** What a window with nowhere to open into refuses with. `openRun`'s sentence, one panel over. */
const NO_CONSOLE = 'This window is not showing a project console to open the conversation into.'

/**
 * Which session a pane is showing — the host registry first, the domain second.
 *
 * `openRun.ts`'s ladder verbatim, and copied for its stated reason: the two halves answer over
 * different intervals and both are needed. The host knows a child that started in this process
 * before the domain has been told; `pane.session` knows a pane restored from `workspace.json`, or
 * one this window has been handed and not yet rendered.
 */
function sessionOf(pane: Pane): string | null {
  return paneSessionId(pane.id) ?? pane.session
}

/** A pane of this project already showing `session`, or `undefined`. Detached panes included. */
function paneShowing(project: Project, session: string): PaneId | undefined {
  const panes: Pane[] = [
    ...project.tabs.flatMap((tab) => Object.values(tab.tree.panes)),
    ...Object.values(project.detached),
  ]
  return panes.find((pane) => sessionOf(pane) === session)?.id
}

/**
 * Wait until the registry actually holds a running child for `session`.
 *
 * The spawn is `TerminalPane`'s, one round trip *after* the pane exists — so a caller that typed
 * into the session the moment `addRow` resolved would be writing at a child that has not been
 * forked yet. `spec_dispatch_to_session` would refuse, having already recorded the link, and the
 * user would be looking at a fresh pane that never got the task.
 *
 * Polled rather than driven off `cide://session-state`, because the interesting transition is
 * *spawn*, which no event names — the states that broadcast are what a live child is doing. The
 * cap is generous and the failure is honest: giving up returns `false` and the caller says the
 * conversation is back but nothing was sent, which is a true sentence about a real state.
 */
async function waitForChild(session: string, timeoutMs = 15_000): Promise<boolean> {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    const live = await sessionApi.exit(session).then(
      (answer) => answer.kind === 'running',
      () => false,
    )
    if (live) return true
    if (Date.now() >= deadline) return false
    await new Promise((resolve) => setTimeout(resolve, 120))
  }
}

/** What `open` did, so the caller can say something true about a partial success. */
export type OpenOutcome = { ok: true } | { ok: false; reason: string }

/**
 * Put `session` on screen, optionally handing `task` back to it.
 *
 * Resolves once the user is looking at the conversation — not once its screen has painted, which
 * is `TerminalPane`'s job and happens a frame or two later. Rejects with a sentence when there is
 * nowhere to open into; `TaskDetailHost`'s `guarded` is what puts that on screen.
 */
export async function openTaskSession(
  session: string,
  options: { project: ProjectId; task?: string | undefined },
): Promise<void> {
  const ws = useWorkspace.getState()
  const boot = ws.boot
  const project = activeProjectOf(boot)
  const target = consolePaneOf(boot)
  if (project === null || target === null) throw new Error(NO_CONSOLE)

  /*
   * Already on screen? Then go there rather than growing a second pane — and *never* spawn:
   * `session_spawn` refuses a plain resume onto an id the registry still holds live, because two
   * panes resuming one conversation is not a thing the CLI supports. Reaching that refusal from
   * a button whose whole job is "show me this" would be cide creating the error itself.
   */
  const already = paneShowing(project, session)
  if (already !== undefined) {
    const why = await revealPane(target.project, already)
    if (why !== null) throw new Error(`That conversation is already open, but ${why}.`)
    if (options.task !== undefined) {
      await specApi.dispatchToSession(options.project, options.task as never, session as never)
    }
    return
  }

  /*
   * A row in the **project console**, not a split of whatever happens to be focused: the focused
   * pane may be an editor in a File tab, and a transcript landing beside a source file is a pane
   * in a place nobody asked for. `tabs[0]` is the pinned console by invariant.
   */
  const tab = project.tabs[0]
  if (tab === undefined) throw new Error(NO_CONSOLE)

  /*
   * The intent is the whole of the spawn. `addRow` records the plan *before* the snapshot that
   * makes the pane renderable — the other order races, and `TerminalPane` mounts, finds no plan,
   * and spawns a fresh `claude` where the user asked for an old conversation.
   */
  const created = await ws.addRow(target.project, tab.id, target.pane, 'after', {
    kind: 'resume',
    session: session as never,
  })

  const why = await revealPane(target.project, created.pane)

  if (options.task !== undefined) {
    // The child is `TerminalPane`'s to fork and it is not there yet. Waiting is what makes the
    // difference between resuming the work and opening an empty pane beside a task that still
    // says nothing happened.
    if (!(await waitForChild(session))) {
      throw new Error(
        'The conversation is back, but its process did not start in time, so the task was not sent to it. Press Resume again once it has.',
      )
    }
    await specApi.dispatchToSession(options.project, options.task as never, session as never)
  }

  if (why !== null) throw new Error(`The conversation was opened, but ${why}.`)
}
