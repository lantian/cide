/**
 * The live half of the project switcher: the open walk, and the three ways it can end.
 *
 * `keys/switcher.ts` is the arithmetic and has no idea what a window is. This is what plugs it
 * into the app — the gate's capture on one side, the workspace store on the other, and a
 * release watcher in between.
 *
 * # There is no keyup route through the key gate
 *
 * `keys/gate.ts` resolves chords on **keydown only**, at two entry points that
 * `check-key-gate.mjs` proves agree for every chord in the modifier × key space. A switcher
 * that commits on release obviously needs to hear a keyup, and the tempting way to get one is
 * a third entry point into the gate — which is exactly the asymmetry the gate exists to catch,
 * and which xterm's handler (called for keydown, keypress *and* keyup) would immediately turn
 * into every command firing twice.
 *
 * So the release is watched **here**, by [`armRelease`], and what it watches is not a chord.
 * The listener consults no keymap, resolves no sequence, names no command and has exactly two
 * outcomes — commit, or cancel. It is a latch on a modifier, not a route into the dispatcher,
 * and it only exists while a walk is open. The gate's own contract is untouched: `decide`
 * still returns `PASS` for every keyup at both entry points.
 *
 * # Listeners live here rather than in the component
 *
 * [`ProjectSwitcher`](../chrome/ProjectSwitcher.tsx) draws the popup and does nothing else.
 * If it owned the keyup listener, a window that had not mounted it — or a render that had not
 * committed yet — would open a walk that nothing could close, and the capture below would
 * swallow Tab for the rest of the session. Arming from the store means the gesture is correct
 * even with no UI at all; the popup is the part the user can see, not the part that works.
 */
import { create } from 'zustand'
import { useWorkspace } from '@/store/workspace'
import { diag, type ProjectId } from '@/ipc/client'
import { currentStroke } from './gate'
import { windowProjectsOf } from './target'
import {
  advance,
  begin,
  capture,
  commit,
  endsHold,
  holdOf,
  selection,
  type Walk,
} from './switcher'

interface SwitcherStore {
  /** The walk in progress, or `null` when the popup is closed. */
  walk: Walk | null
}

export const useSwitcher = create<SwitcherStore>(() => ({ walk: null }))

/** Torn down when the walk ends, however it ends. `null` while closed. */
let disarm: (() => void) | null = null

/**
 * Start listening for the release, and for the ways a release never comes.
 *
 * All three listeners are on `window`/`document` in the capture phase and all three are
 * removed together, because the failure this guards is a walk that outlives its own gesture:
 *
 * * **keyup** — the ordinary path, and the one the user actually performs: let go of Ctrl and
 *   the highlighted project opens. Whether the hold is over is `endsHold`'s decision, off the
 *   event alone — the key that came *up* first, and only then the modifier flags, because
 *   WebKitGTK still reports Ctrl as down in the very event that releases it. Read its note
 *   before simplifying this back to `!stillHeld`; that spelling leaves the popup up for ever
 *   on the platform this ships on.
 * * **blur** — Alt+Tab to another application while Ctrl is down. The keyup is delivered to
 *   whatever took focus and never arrives here, so without this the popup stays up for ever
 *   and the capture eats every Tab the user types afterwards.
 * * **visibilitychange** — the same loss with no `blur`: the window is hidden (workspace
 *   switch, session lock) rather than defocused.
 *
 * Both of the lost-keyup cases **cancel** rather than commit. The user did not release the
 * key here; activating a project because they alt-tabbed away would be the app acting on a
 * gesture that was interrupted, not completed.
 */
function armRelease(): void {
  disarm?.()

  const onKeyUp = (ev: KeyboardEvent): void => {
    const walk = useSwitcher.getState().walk
    if (walk === null) return
    if (endsHold(walk.hold, ev)) commitWalk()
  }
  const onLost = (): void => {
    if (useSwitcher.getState().walk !== null) cancelWalk()
  }
  const onVisibility = (): void => {
    if (document.visibilityState === 'hidden') onLost()
  }

  window.addEventListener('keyup', onKeyUp, true)
  window.addEventListener('blur', onLost)
  document.addEventListener('visibilitychange', onVisibility)

  disarm = () => {
    window.removeEventListener('keyup', onKeyUp, true)
    window.removeEventListener('blur', onLost)
    document.removeEventListener('visibilitychange', onVisibility)
    disarm = null
  }
}

/** Close the popup and drop the listeners. The one place `walk` goes back to `null`. */
function close(): void {
  disarm?.()
  useSwitcher.setState({ walk: null })
}

/**
 * Escape, or a lost hold. Closes and **writes nothing** — the MRU stack is untouched, because
 * the only function that produces a new order is `commit`, and cancelling does not call it.
 */
export function cancelWalk(): void {
  close()
}

/** Release. Activate the selection; the stack reorders as a consequence, not as a side effect. */
export function commitWalk(): void {
  const walk = useSwitcher.getState().walk
  close()
  if (walk === null) return

  const ws = useWorkspace.getState()
  const { selected, order } = commit(walk, windowProjectsOf(ws.boot))
  if (selected === null) {
    // The highlighted project closed while the walk was open — in another window, or because
    // a snapshot landed mid-gesture. Reported rather than silent: a released Ctrl+Tab that
    // does nothing otherwise reads as the key being broken.
    void diag.log('[cide] project switcher: the selected project is no longer open')
    return
  }
  /*
   * Order first, activation second, and both on the release — never on a Tab.
   *
   * `commit` is the only function in `switcher.ts` that produces a new stack, so "the reorder
   * happens once, at the end" is a property of the code rather than a rule to remember. The
   * eager write is not a second opinion: `store/workspace.ts` recomputes the same order from
   * the snapshot the activation triggers. See `rememberMru` for why the early copy earns its
   * place.
   */
  ws.rememberMru(order as readonly ProjectId[])
  void ws.activateProject(selected as ProjectId)
}

/**
 * `project.switcher.next` / `.prev`. The only entry point.
 *
 * The first press does one of three things, decided by `begin`: nothing at all with fewer than
 * two projects, an immediate switch when no modifier is being held (the palette ran it), or an
 * open walk that waits for the release.
 */
export function startProjectSwitch(step: 1 | -1): void {
  const ws = useWorkspace.getState()
  const open = windowProjectsOf(ws.boot)
  if (open.length < 2) {
    void diag.log('[cide] project switcher: this window holds fewer than two projects')
    return
  }

  const walking = useSwitcher.getState().walk
  if (walking !== null) {
    // A second dispatch while the popup is up. Only reachable when the user has bound the
    // command to a chord the capture does not claim — the capture takes Tab itself, so the
    // ordinary Ctrl+Tab never gets this far — but it has to mean "one more step" either way.
    useSwitcher.setState({ walk: advance(walking, step) })
    return
  }

  // The stack is reconciled against the live strip on every snapshot, so this order already
  // contains exactly the projects this window can activate.
  const outcome = begin(ws.mru, holdOf(currentStroke()), step)
  if (outcome.kind === 'nothing') return
  if (outcome.kind === 'activate') {
    void ws.activateProject(outcome.id as ProjectId)
    return
  }
  useSwitcher.setState({ walk: outcome.walk })
  armRelease()
}

/**
 * The gate's `capture` hook: what an open walk wants done with one stroke, before the keymap.
 *
 * Returns `true` when the stroke was consumed and must reach neither the keymap nor the PTY.
 * Closed, it is a `null` check and a `false` — this runs on every keystroke in the app.
 */
export function switcherCapture(stroke: string): boolean {
  const walk = useSwitcher.getState().walk
  if (walk === null) return false

  const action = capture(walk, stroke)
  if (action.kind === 'advance') {
    useSwitcher.setState({ walk: advance(walk, action.step) })
  } else if (action.kind === 'cancel') {
    cancelWalk()
  }
  return action.consumed
}

/** The highlighted project id, for the popup. `null` when nothing is open. */
export function useSwitcherSelection(): string | null {
  return useSwitcher((s) => (s.walk === null ? null : selection(s.walk)))
}
