/**
 * *Open* — put a headless run's transcript into a pane of this window's project console. (M18)
 *
 * The Agents panel's third run gesture, and the only one that does not end in a Tauri command.
 * It is a **frontend** gesture on purpose, and the reason is a live bug that arrives by a door
 * this feature would otherwise have opened a second time.
 *
 * # The door, and why it has to stay shut
 *
 * A pane that shows a session it did not spawn must never kill that session when it closes.
 * `store/workspace.ts::closePane` ends with `if (session) await sessionApi.kill(session)`, and
 * the only thing standing between that line and somebody else's child is `PaneHost.mirrored` —
 * set in `panes/TerminalPane.tsx`'s **mirror branch**, the one that runs when
 * `takeSpawnPlan(paneId)` answers `{ kind: 'mirror', session }`. Closing a `claude.mirror` pane
 * without it killed the conversation in the pane being mirrored; that is what shipped, and it is
 * what the flag now prevents.
 *
 * A command that builds the pane **in the domain** — which is what a short-lived
 * `agent_open_pane` did, since deleted — produces one that reaches the webview over
 * `cide://workspace-changed` like any other, so there is no spawn plan for it, the mirror branch
 * never runs, `mirrored` is never set, and the pane adopts the run's session through
 * `TerminalPane`'s other route — the `sessionIsLive(domainSession)` check, which exists for a
 * re-docked or restored pane that genuinely owns what it holds. The pane then looks, to
 * `closePane`, exactly like a pane that spawned its own child. **Closing the pane you were
 * reading an agent in would have killed that agent mid-turn**, silently, with the run row simply
 * moving to Recent as though it had finished.
 *
 * # The fix is the gesture the split machinery already has
 *
 * Not a second ownership signal — two answers to "does this pane own its child" is how the first
 * one gets out of date. `SplitIntent::Mirror` *is* this gesture ("a second sink on an existing
 * session, no new process"), and its path is correct by construction: `addRow` calls
 * `rememberSpawnPlan(created.pane, created.intent)` **before** waiting for the snapshot that
 * makes the pane renderable, so `TerminalPane` finds the plan, takes the mirror branch, sets
 * `mirrored`, adopts the id and spawns nothing. Every property the panel wants falls out of it,
 * including the one nobody asked for: sinks are pane-keyed, so two panes — or two windows — may
 * mirror one run without either of them owning it.
 *
 * `PaneKind` stays `claude` and no new `SplitIntent` variant is added. An `agent` kind would need
 * arms in six modules to express a fact the run already carries, and would flip
 * `claudePaneFocused` to false — taking `claude.fork`, `claude.mirror` and `claude.restart` away
 * from a pane where all three still make sense.
 *
 * # What happens afterwards, which is deliberately nothing
 *
 * **When the run exits with its pane open**, nothing here runs at all: `cide://session-state`
 * carries `Exited { code }` to `TerminalPane`, which writes the `— exited —` marker and offers a
 * restart, while the roster's own broadcast moves the row to Recent. Two surfaces, one truth. The
 * panel must **never** close a pane because its run finished — the transcript is the record of
 * what the agent did, and is the reason the pane was opened.
 *
 * **When the pane is closed while the run continues**, the pane closes its *view*: `mirrored` is
 * set, so `closePane` kills nothing, the child keeps working, the row stays where it was, and
 * pressing Open again brings the whole transcript back — `PtySession` feeds its vt100 mirror
 * independently of sinks, which is why a run that was headless for twenty minutes opens with its
 * screen intact.
 *
 * # Not exported through `index.ts`, and not imported by `model.ts`
 *
 * `ui/scripts/check-agents.mjs` compiles `model.ts` **alone**, with a bare `tsc` and no tsconfig.
 * This module reaches `@/store/workspace` and `@/ipc/client`, so a barrel entry — or an import in
 * the other direction — would put the whole app graph one hop from a check whose entire value is
 * that it needs none of it. Import it by path, from the store.
 */
import { revealPane } from '@/editor/revealPane'
import type { Pane, PaneId, Project } from '@/ipc/client'
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

/**
 * A pane of this project already showing `session`, or `undefined`.
 *
 * Every tab, plus the panes torn out into windows of their own — `revealPane` handles both, and a
 * run being read in a detached pane is still a run that is already open. Written over the mirror
 * rather than over the host registry because a host only exists for a pane **this** window has
 * rendered, and the answer must not depend on which window is asking.
 */
function paneShowing(project: Project, session: string): PaneId | undefined {
  const panes: Pane[] = [
    ...project.tabs.flatMap((tab) => Object.values(tab.tree.panes)),
    ...Object.values(project.detached),
  ]
  return panes.find((pane) => sessionOf(pane) === session)?.id
}

/**
 * Show `run`'s transcript, adding a pane for it only if nothing is showing it yet.
 *
 * Rejects with a sentence when it cannot; `AgentsPanelHost`'s `guarded` is what puts that on
 * screen. Resolves once the pane exists and the user is looking at it — not once its screen has
 * painted, which is `TerminalPane`'s job and happens a frame or two later.
 */
export async function openRunInPane(run: RunView): Promise<void> {
  /*
   * 1. The gate, asked again.
   *
   * `RunRow` withholds the Open control entirely when `canOpen` is false — a queued run has no
   * session to mirror, and a disabled button with no sentence is a dead control wearing grey —
   * but the roster moves under the panel on every `cide://agents-changed`, and a click can land
   * against a row drawn from the previous one. `canOpen`'s own answer is reused rather than
   * restated: a second copy of "is there a transcript to attach to" is how a pane opens on
   * nothing. The `session === null` test beside it is not redundant — it is what narrows the
   * type for the intent below, and `canOpen` already implies it.
   */
  const session = run.session
  if (session === null || !canOpen(run)) return

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
   * 2. Already open? Then go there, rather than growing a second pane.
   *
   * Two panes mirroring one run is legal and costs nothing — that is what pane-keyed sinks bought
   * — but it is not what one click asked for, and the second press of a button whose first press
   * scrolled a pane into view somewhere the user was not looking would keep adding rows until the
   * tab hit `MAX_MEMBERS`. `revealPane` is the whole of "go there": it activates the project and
   * its tab, clears a maximize that would hide the pane, moves the domain's focus, raises the
   * window when the pane is in another one, and puts the caret in the terminal — in that order,
   * which is load-bearing and argued at length where it lives.
   */
  const open = paneShowing(project, session)
  if (open !== undefined) {
    const why = await revealPane(target.project, open)
    // `revealPane` never throws and reports a reason instead, because for a *mention* the send
    // had already landed and a red toast would have been a lie. Here the reveal is the entire
    // gesture, so a reason means nothing happened, and saying so is the point.
    if (why !== null) throw new Error(`This run is already open, but ${why}.`)
    return
  }

  /*
   * 3. A row in the **project console**, not a split of the focused pane.
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
   * 4. The mirror itself.
   *
   * The intent is the whole of it. `addRow` records the spawn plan before the snapshot that makes
   * the pane renderable — the other order races, and `TerminalPane` mounts, finds no plan, and
   * spawns a fresh `claude` where the user asked to watch an existing one — and the mirror branch
   * on the far side adopts the id, marks the host `mirrored`, and starts no process at all.
   */
  const created = await ws.addRow(target.project, tab.id, target.pane, 'after', {
    kind: 'mirror',
    session,
  })

  /*
   * And then take the user to it, for the reason step 3 exists: the console is routinely not the
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
