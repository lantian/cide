/**
 * Running one `docker compose` verb against a file, in a pane. (M48)
 *
 * # One function, three surfaces
 *
 * The file tree's context menu, the editor's gutter and the command palette all end here. They
 * have to: the gesture is the same in all three and the ordering below is delicate in exactly one
 * place, so a second copy would be a second chance to get that one place wrong.
 *
 * # Why a pane, and not a button that waits
 *
 * `cide_ipc::docker::ComposeAction`'s doc carries the argument in full. In short: `up` on a stack
 * that pulls images is unbounded, its progress *is* the answer, and a failure is a sentence
 * ("port is already allocated") the user has to read. The panel's stack buttons wait for a
 * bounded verb and report the board; this road shows the run.
 *
 * What comes back is an ordinary shell pane, which is the whole point — the job watcher lights
 * the pane dot when the run ends, Ctrl+C reaches the child, and detaching it into a window loses
 * nothing.
 */
import { docker as dockerApi, session as sessionApi } from '@/ipc/client'
import { notifyFailure } from '@/chrome/notices'
import { activeProjectIdOf, activeTabOf } from '@/keys/target'
import { FALLBACK } from '@/panes/sessionSink'
import { useWorkspace } from '@/store/workspace'
import type { ComposeVerb } from './composeModel'

/**
 * Ask Compose to do one thing to one file, and put the run on screen.
 *
 * Resolves once the pane exists, **not** once the run finishes: there is nothing here to wait
 * for, and a caller that awaited a `docker compose pull` would be a menu item that stays pressed
 * for ten minutes.
 *
 * Every failure before the pane is a notice — no Compose on this machine, no project open — and
 * every failure after it is text in the pane, where the user is already looking.
 */
export async function runCompose(
  file: string,
  action: ComposeVerb,
  /**
   * Narrow the run to these services, or none for the whole file.
   *
   * Only the editor's gutter passes any: it draws a marker per service, and Compose takes service
   * names on every one of the six verbs (checked against the plugin — see
   * `cide_docker::compose::argv_for`). The tree, the palette and the file-level marker all mean
   * the whole stack.
   */
  services: readonly string[] = [],
): Promise<void> {
  /*
   * The plan first, and the split second.
   *
   * That order is deliberate: `composePlan` rejects when there is no Compose on this machine,
   * with the sentence that names what to install — and doing it first means that refusal arrives
   * *instead of* a pane rather than inside an empty one. The reverse order would leave a pane
   * titled `compose up` that printed nothing and explained nothing.
   */
  let plan
  try {
    plan = await dockerApi.composePlan(file, action, services)
  } catch (reason) {
    notifyFailure(reason)
    return
  }

  /*
   * The focused pane of the focused tab of the focused project, read at *this* moment rather
   * than captured when the menu opened: a plan round trip is long enough for the user to have
   * clicked elsewhere, and splitting a pane that is no longer on screen is how a run ends up in
   * a tab nobody is looking at. `keys/target.ts` carries the same rule for keyboard commands.
   */
  const boot = useWorkspace.getState().boot
  const project = activeProjectIdOf(boot)
  const tab = activeTabOf(boot)
  const pane = tab?.tree.focused ?? null
  if (project === null || tab === null || pane === null) {
    // Not a silent no-op: the user pressed something. A compose run needs a pane, a pane needs a
    // tab and a tab needs a project, and "no project is open" is the whole of the answer.
    notifyFailure('A Compose run opens in a pane, so a project has to be open first.')
    return
  }

  /*
   * **The session first, the pane second**, and that order is the whole of the M50 fix.
   *
   * The first cut did the opposite: split a pane carrying the argv on the intent, and parked that
   * argv in `layout/spawnPlans.ts` for the moment between the split committing and `TerminalPane`
   * mounting. That is the road `forkPrimary` and `mirror` take, and it has a race those two
   * survive and this one did not — a plan is recorded after `pane_split` *resolves*, while the
   * pane it is for can render as soon as the `workspace_changed` broadcast lands, which Rust
   * sends before the command returns. A mirror that loses its plan falls through to adopting a
   * held session and looks fine. A compose pane that lost its plan fell through to `specFor`'s
   * shell arm and opened a **login shell**: reported as "compose up does nothing, only opens an
   * empty terminal", which is exactly what it was.
   *
   * Spawning first makes the run exist before anything can render, and `SplitIntent::Adopt` names
   * it in `Pane::session`, which is **durable** — so nothing has to survive any gap.
   */
  let session
  try {
    session = await sessionApi.spawn({
      program: plan.program,
      args: [...plan.args],
      cwd: plan.workingDir,
      project,
      /*
       * `sessionSink`'s own fallback, not a second guess at the same question.
       *
       * There is no pane to measure yet — that is the price of spawning first, and it is the
       * right price because the alternative is the race above. It costs nothing: the pane
       * resizes the pty the moment it attaches (`syncSize`), and nothing has been printed before
       * then. Reusing the constant means "what size when we cannot measure" has one answer in
       * this app rather than two that drift.
       */
      geometry: FALLBACK,
    })
  } catch (reason) {
    notifyFailure(reason)
    return
  }

  /*
   * A **row below** (`'col'`, `'after'`) and not a tile beside.
   *
   * The docker panel's streams split sideways, for the reason it states — a log follow wants
   * width for long lines. This one is different in what it splits *from*: the user is looking at
   * the compose file in an editor, and halving that editor to show ten lines of `Container
   * shop-db-1  Started` takes away the thing they were reading.
   */
  await useWorkspace
    .getState()
    .splitPane(project, tab.id, pane, 'col', 'after', {
      kind: 'adopt',
      session,
      title: plan.title,
    })
}
