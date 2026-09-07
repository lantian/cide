/**
 * *Open* — put the **real harness** on a run's conversation, in a pane of this window's project
 * console. (M18, rewritten in M42)
 *
 * The Agents panel's third run gesture. It asks Rust one question — `agentRuns.open` — and builds
 * the pane itself through the split machinery with the intent the answer names. Three answers:
 *
 * * **`mirror`** — the run's child is alive. A second sink on its session, no new process: for a
 *   `claude` run that is the real TUI mid-turn, for `opencode` the rendered stream of a turn in
 *   flight. The attach asks for the mirror's retained scrollback in front of the screen
 *   (`sessionSink.ts`, `historyRequest`), so a run that has been printing for twenty minutes
 *   opens with its transcript rather than its last two dozen lines.
 * * **`continue`** — the child is gone and the harness can be put back on the conversation:
 *   `claude --resume <id>` in the run's worktree, or the opencode TUI on its `ses_…`. A new
 *   process, spawned by the pane, which therefore owns it. This is what replaced the row-by-row
 *   replay of a dead mirror — an Ink TUI's scrollback rendered at the headless 80×24, "a hardly
 *   concatenated log" in the words of the report — and it is what makes a finished run readable
 *   *and* continuable, from the harness's own rendering, after a cide restart included.
 * * **`unavailable`** — nothing to show, and the sentence says why. Thrown, so `guarded` puts it
 *   on screen: a press that resolves to nothing is the dead control this panel exists to make
 *   unrepresentable.
 *
 * # Why the pane is still built here and not by the command
 *
 * A pane that shows a session it did not spawn must never kill that session when it closes.
 * `store/workspace.ts::closePane` ends with `if (session) await sessionApi.kill(session)`, and
 * the only thing standing between that line and somebody else's child is `PaneHost.mirrored` —
 * set in `panes/TerminalPane.tsx`'s **mirror branch**, the one that runs when
 * `takeSpawnPlan(paneId)` answers `{ kind: 'mirror' }`. A pane built **in the domain** — which
 * is what a short-lived `agent_open_pane` did, since deleted — arrives in the webview over
 * `cide://workspace-changed` like any other, so there is no spawn plan for it, the mirror branch
 * never runs, `mirrored` is never set, and the pane adopts the run's session through the
 * `sessionIsHeld` route that exists for a re-docked pane which genuinely owns what it holds.
 * **Closing the pane you were reading an agent in would have killed that agent mid-turn.** That
 * shipped once; `addRow` calling `rememberSpawnPlan` *before* the snapshot that makes the pane
 * renderable is what makes the flag correct by construction, for a mirror and — the other way
 * round — for a continuation, whose pane spawned the child and must end it.
 *
 * So the command answers and the gesture builds; `cmd/agents.rs` carries the long version beside
 * the deleted `agent_open_pane`, and the same rule still holds: any route that hands an existing
 * session to a new pane goes through the spawn-plan path.
 *
 * # What the pane keeps
 *
 * Both intents carry the conversation (`HarnessSession`: harness, id, cwd) into `Pane.continues`,
 * and that is the whole reason a mirror carries one it does not need yet: when the run's child
 * later ends in a pane somebody is reading, the exit bar's *Resume this conversation* and a
 * restart re-open the conversation from the run's directory — not a fresh `claude` in the project
 * root over a transcript filed under the worktree. A restart of cide does the same through
 * `plan_restore`.
 *
 * # What happens afterwards, which is deliberately nothing
 *
 * **When a mirrored run exits with its pane open**, nothing here runs at all:
 * `cide://session-state` carries `Exited { code }` to `TerminalPane`, which writes the `— exited
 * —` marker and offers the way back, while the roster's own broadcast moves the row to Recent.
 * The panel must **never** close a pane because its run finished — the transcript is the record
 * of what the agent did, and is the reason the pane was opened.
 *
 * **When the pane is closed while the run continues**, the pane closes its *view*: `mirrored` is
 * set, so `closePane` kills nothing, the child keeps working, and pressing Open again brings the
 * whole transcript back. A **continued** pane is the opposite by design: closing it ends the
 * harness process the pane started — and frees the conversation for the row's own Resume, which
 * the registry refuses while a pane is typing into it.
 *
 * # Not exported through `index.ts`, and not imported by `model.ts`
 *
 * `ui/scripts/check-agents.mjs` compiles `model.ts` **alone**, with a bare `tsc` and no tsconfig.
 * This module reaches `@/store/workspace` and `@/ipc/client`, so a barrel entry — or an import in
 * the other direction — would put the whole app graph one hop from a check whose entire value is
 * that it needs none of it. Import it by path, from the store.
 */
import { revealPane } from '@/editor/revealPane'
import { agentRuns, type HarnessSession, type Pane, type PaneId, type Project } from '@/ipc/client'
import { activeProjectOf, consolePaneOf } from '@/keys/target'
import { paneSessionId } from '@/layout/paneHosts'
import { useWorkspace } from '@/store/workspace'
import { canOpen, type RunView } from './model'

/**
 * What a window with nothing to open into refuses with.
 *
 * Thrown rather than returned: `AgentsPanelHost` funnels every run gesture through `guarded`,
 * which puts the rejection on screen via `notifyFailure`, and a press that resolves to nothing at
 * all is the dead control this panel's whole design exists to make unrepresentable.
 */
const NO_CONSOLE = 'This window is not showing a project console to open the run into.'

/**
 * Which session a pane is showing, by `PaneFrame`'s rule verbatim — the host first, the domain
 * second.
 *
 * Copied rather than approximated because the two halves answer over different intervals and
 * both are needed here: the host registry knows a child that started in this process before the
 * domain has been told (`TerminalPane` binds one round trip later), and `pane.session` knows a
 * pane restored from `workspace.json`, or one this window has been handed and not yet rendered.
 * A mirror pane is set both ways — `cmd::pane::pane_for` puts the session straight into the
 * `Pane` for a `Mirror` intent — so either half alone would still find one; the ladder is what
 * makes this agree with the tab badges and the pane markers, which is what stops "already open"
 * being answered two ways by two surfaces.
 */
function sessionOf(pane: Pane): string | null {
  return paneSessionId(pane.id) ?? pane.session
}

/** Whether two conversation records name the same conversation of the same harness. */
function sameConversation(a: HarnessSession | null, b: HarnessSession | null): boolean {
  return a !== null && b !== null && a.harness === b.harness && a.id === b.id
}

/**
 * A pane of this project already showing `session` — or, when the run's conversation is known,
 * a pane that was opened onto that conversation — or `undefined`.
 *
 * Every tab, plus the panes torn out into windows of their own — `revealPane` handles both, and a
 * run being read in a detached pane is still a run that is already open. Written over the mirror
 * rather than over the host registry because a host only exists for a pane **this** window has
 * rendered, and the answer must not depend on which window is asking.
 *
 * The conversation half matters for an opencode continuation, whose pane session is minted at
 * the spawn and shares nothing with the run's: only `Pane.continues` says it is the same
 * conversation, and a second press of Open must reveal that pane rather than spawn a second TUI
 * on one session.
 */
function paneShowing(
  project: Project,
  session: string | null,
  conversation: HarnessSession | null,
): PaneId | undefined {
  const panes: Pane[] = [
    ...project.tabs.flatMap((tab) => Object.values(tab.tree.panes)),
    ...Object.values(project.detached),
  ]
  return panes.find(
    (pane) =>
      (session !== null && sessionOf(pane) === session) ||
      sameConversation(pane.continues, conversation),
  )?.id
}

/**
 * Show `run`'s conversation, adding a pane for it only if nothing is showing it yet.
 *
 * Rejects with a sentence when it cannot; `AgentsPanelHost`'s `guarded` is what puts that on
 * screen. Resolves once the pane exists and the user is looking at it — not once its screen has
 * painted, which is `TerminalPane`'s job and happens a frame or two later.
 */
export async function openRunInPane(run: RunView): Promise<void> {
  /*
   * 1. The gate, asked again.
   *
   * `RunRow` withholds the Open control entirely when `canOpen` is false — a queued run has
   * nothing to show, and a disabled button with no sentence is a dead control wearing grey —
   * but the roster moves under the panel on every `cide://agents-changed`, and a click can land
   * against a row drawn from the previous one. `canOpen`'s own answer is reused rather than
   * restated: a second copy of "is there anything to open" is how a pane opens on nothing.
   */
  if (!canOpen(run)) return

  const ws = useWorkspace.getState()
  const boot = ws.boot
  /*
   * The project and its console pane, both from this window's own mirror.
   *
   * `consolePaneOf` is `tab.console`'s (Ctrl+1) helper and `claudeTargetOf`'s fallback branch —
   * "the conversation the project is *about*" — and it is reused instead of a second walk to
   * `tabs[0]` because a second walk is how two surfaces end up disagreeing about which pane the
   * console is. It resolves the project this window is showing, which is the project the panel is
   * drawing a roster for: `App.tsx` attaches `agentsStore` to the same id.
   */
  const target = consolePaneOf(boot)
  const project = activeProjectOf(boot)
  if (target === null || project === null) throw new Error(NO_CONSOLE)

  /*
   * 2. What Open means for this run right now — Rust's answer, from the two facts only Rust
   * has: whether the registry still holds a live child for the run, and whether the harness can
   * put itself back on the conversation from the run's directory. Asked *before* the
   * already-open check because the answer carries the conversation that check matches on.
   */
  const plan = await agentRuns.open(target.project, run.run)
  if (plan.kind === 'unavailable') throw new Error(plan.reason)
  const session = plan.kind === 'mirror' ? plan.session : null
  const conversation = plan.kind === 'mirror' ? plan.continues : plan.conversation

  /*
   * 3. Already open? Then go there, rather than growing a second pane.
   *
   * Two panes mirroring one run is legal and costs nothing — that is what pane-keyed sinks bought
   * — but it is not what one click asked for, and the second press of a button whose first press
   * scrolled a pane into view somewhere the user was not looking would keep adding rows until the
   * tab hit `MAX_MEMBERS`. For a continuation it is more than tidiness: `session_spawn` refuses a
   * second `claude --resume` on a live conversation, and a second opencode TUI on one session is
   * two processes writing one transcript. `revealPane` is the whole of "go there": it activates
   * the project and its tab, clears a maximize that would hide the pane, moves the domain's
   * focus, raises the window when the pane is in another one, and puts the caret in the terminal
   * — in that order, which is load-bearing and argued at length where it lives.
   */
  const open = paneShowing(project, session, conversation)
  if (open !== undefined) {
    const why = await revealPane(target.project, open)
    // `revealPane` never throws and reports a reason instead, because for a *mention* the send
    // had already landed and a red toast would have been a lie. Here the reveal is the entire
    // gesture, so a reason means nothing happened, and saying so is the point.
    if (why !== null) throw new Error(`This run is already open, but ${why}.`)
    return
  }

  /*
   * 4. A row in the **project console**, not a split of the focused pane.
   *
   * The focused pane may be an editor in a File tab, and a subagent's transcript landing beside a
   * source file is a pane in a place nobody asked for. `tabs[0]` is the pinned console by
   * invariant — `open_tab` refuses a second `ClaudeHome` and `close_tab` refuses index 0 — so
   * this is the same tab `consolePaneOf` just answered for, and the `undefined` arm fires only
   * for a window with no project, which the check above has already excluded.
   *
   * A full-width row rather than a tile beside something: a transcript is read down the page, and
   * this is what splitting downwards has always produced.
   */
  const tab = project.tabs[0]
  if (tab === undefined) throw new Error(NO_CONSOLE)

  /*
   * 5. The pane itself — a mirror or a continuation, and the intent is the whole of it.
   *
   * `addRow` records the spawn plan before the snapshot that makes the pane renderable — the
   * other order races, and `TerminalPane` mounts, finds no plan, and spawns a fresh `claude`
   * where the user asked to watch an existing conversation. On the far side the mirror branch
   * adopts the id, marks the host `mirrored` and starts no process; the continue branch leaves
   * `mirrored` unset and lets `session_spawn` put the harness on the conversation.
   */
  const created = await ws.addRow(
    target.project,
    tab.id,
    target.pane,
    'after',
    plan.kind === 'continue'
      ? { kind: 'continue', conversation: plan.conversation }
      : plan.continues !== null
        ? { kind: 'mirror', session: plan.session, continues: plan.continues }
        : { kind: 'mirror', session: plan.session },
  )

  /*
   * And then take the user to it, for the reason step 4 exists: the console is routinely not the
   * tab in front of them, and a row added to a tab nobody is looking at is the same "nothing
   * happened" the tab choice was made to avoid. `addRow` has already focused the pane in the
   * domain, so this is mostly the tab switch and the caret.
   *
   * A reason here is worth saying but is not a failure of the gesture — the pane exists and the
   * user can reach it — so it names both halves rather than reading as a refusal.
   */
  const why = await revealPane(target.project, created.pane)
  if (why !== null) {
    throw new Error(`The run was opened in the project console, but ${why}.`)
  }
}
