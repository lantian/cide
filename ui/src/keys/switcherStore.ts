/**
 * The live half of the held-modifier switchers: the open walk, and the three ways it can end.
 *
 * `keys/switcher.ts` is the arithmetic and has no idea what a window is. This is what plugs it
 * into the app — the gate's capture on one side, the workspace store on the other, and a
 * release watcher in between.
 *
 * # Two switchers, one store, and why that is not a tidiness argument
 *
 * Ctrl+Tab walks the active project's **tabs**; Ctrl+` walks this window's **projects**. The
 * arithmetic is shared because it is pure and identical, but the thing that must not be
 * duplicated is [`armRelease`] below. Two stores would mean two capture-phase `keyup` latches,
 * two `disarm` closures and two independent `walk` states — and the gate has exactly *one*
 * `capture` hook (`keys/gate.ts`, wired in `keys/useKeyGate.ts`), so it could only ever consult
 * one of them. The loser's popup would then be immortal: swallowing nothing, closing never, and
 * the winner still eating Tab. That is the failure this module's shape exists to prevent, and
 * `check-switcher.mjs` counts the `keyup` registrations to keep it prevented.
 *
 * So there is one walk at a time — which is not a restriction but the model, since a user
 * cannot hold two held-modifier gestures at once — and a [`SwitchTarget`] captured when it
 * opens says what is being walked, where the order comes from and what a commit does with it.
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
 * [`Switcher`](../chrome/Switcher.tsx) draws the popup and does nothing else. If it owned the
 * keyup listener, a window that had not mounted it — or a render that had not committed yet —
 * would open a walk that nothing could close, and the capture below would swallow Tab for the
 * rest of the session. Arming from the store means the gesture is correct even with no UI at
 * all; the popup is the part the user can see, not the part that works.
 */
import { create } from 'zustand'
import { useWorkspace } from '@/store/workspace'
import { revealPane } from '@/editor/revealPane'
import { diag, type ProjectId, type TabId } from '@/ipc/client'
import { currentStroke } from './gate'
import { activeProjectIdOf, windowProjectsOf } from './target'
import {
  advance,
  begin,
  capture,
  commit,
  endsHold,
  openingOf,
  reconcile,
  type Walk,
} from './switcher'

/**
 * What a walk is walking — the whole of what the store knows about projects and tabs.
 *
 * Captured when the popup opens and read again at the commit, so a snapshot landing mid-gesture
 * is seen: `live` is deliberately a function rather than a frozen array, because `commit`'s
 * reconciliation is the thing that stops a released Ctrl activating something that has closed.
 */
export interface SwitchTarget {
  /** What the popup is drawing. The only field the UI reads. */
  readonly kind: 'project' | 'tab'
  /** The project whose tabs are being walked; `null` for the project switcher. */
  readonly project: ProjectId | null
  /** The ids that exist **now**, in strip order. Asked again at the commit. */
  readonly live: () => readonly string[]
  /** The remembered order, frozen into the walk when it opens. */
  readonly order: () => readonly string[]
  /** Record the order the commit produced, before the activation round trip lands. */
  readonly remember: (order: readonly string[]) => void
  /** Go there. */
  readonly activate: (id: string) => void
  /** The diagnostic for "fewer than two of these, so there is nowhere to switch to". */
  readonly empty: string
}

interface SwitcherStore {
  /** The walk in progress, or `null` when the popup is closed. */
  walk: Walk | null
  /** What that walk is walking. `null` exactly when `walk` is. */
  target: SwitchTarget | null
}

export const useSwitcher = create<SwitcherStore>(() => ({ walk: null, target: null }))

/** Torn down when the walk ends, however it ends. `null` while closed. */
let disarm: (() => void) | null = null

/**
 * Start listening for the release, and for the ways a release never comes.
 *
 * All three listeners are on `window`/`document` in the capture phase and all three are
 * removed together, because the failure this guards is a walk that outlives its own gesture:
 *
 * * **keyup** — the ordinary path, and the one the user actually performs: let go of Ctrl and
 *   the highlighted entry opens. Whether the hold is over is `endsHold`'s decision, off the
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
 * key here; activating something because they alt-tabbed away would be the app acting on a
 * gesture that was interrupted, not completed.
 *
 * There is exactly one of these in the module, and that is a checked property rather than a
 * habit — see the note at the top about what two latches would do to one `capture` hook.
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
  useSwitcher.setState({ walk: null, target: null })
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
  const { walk, target } = useSwitcher.getState()
  close()
  if (walk === null || target === null) return

  const { selected, order } = commit(walk, target.live())
  if (selected === null) {
    // The highlighted entry went away while the walk was open — a project closed in another
    // window, a tab closed by a snapshot that landed mid-gesture. Reported rather than silent:
    // a released Ctrl+Tab that does nothing otherwise reads as the key being broken.
    void diag.log(`[cide] ${target.kind} switcher: the selection is no longer open`)
    return
  }
  /*
   * Order first, activation second, and both on the release — never on a Tab.
   *
   * `commit` is the only function in `switcher.ts` that produces a new stack, so "the reorder
   * happens once, at the end" is a property of the code rather than a rule to remember. The
   * eager write is not a second opinion: `store/workspace.ts` recomputes the same order from
   * the snapshot the activation triggers. See `rememberMru` for why the early copy earns its
   * place, and why the same argument applies to `rememberTabMru` beside it.
   */
  target.remember(order)
  target.activate(selected)
}

/**
 * Open a walk, or switch immediately when nothing is being held.
 *
 * The one entry point both switchers go through, and the private half of the pair of exported
 * wrappers below. The first press does one of three things, decided by `begin`: nothing at all
 * with fewer than two entries, an immediate switch when no modifier is being held (the palette
 * ran it), or an open walk that waits for the release.
 */
function startSwitch(target: SwitchTarget, step: 1 | -1): void {
  const live = target.live()
  if (live.length < 2) {
    void diag.log(target.empty)
    return
  }

  const walking = useSwitcher.getState().walk
  if (walking !== null) {
    // A second dispatch while a popup is up. Only reachable when the stroke is one the capture
    // does not claim — the capture takes the walk's own key, so the ordinary second Ctrl+Tab
    // never gets this far — but it has to mean "one more step" either way. Note that it steps
    // *the open walk*, whatever that is walking: pressing Ctrl+Tab during a project walk moves
    // the project selection rather than opening a second popup, which is the only coherent
    // answer when one hold is being held.
    useSwitcher.setState({ walk: advance(walking, step) })
    return
  }

  /*
   * Reconciled against the live list before it is frozen, rather than trusted.
   *
   * Both stacks are already reconciled on every snapshot by `store/workspace.ts`, so this is
   * almost always the identity — but "almost always" is how a walk comes to be frozen over an
   * id that no longer exists, and the popup would then draw a blank row and the release would
   * activate nothing. `reconcile` is the same function the snapshot rule uses, so this cannot
   * be a second opinion about what the list is.
   */
  const order = reconcile(target.order(), live)
  const outcome = begin(order, openingOf(currentStroke()), step)
  if (outcome.kind === 'nothing') return
  if (outcome.kind === 'activate') {
    target.activate(outcome.id)
    return
  }
  useSwitcher.setState({ walk: outcome.walk, target })
  armRelease()
}

/** `project.switcher.next` / `.prev` — Ctrl+` by default, and the palette. */
export function startProjectSwitch(step: 1 | -1): void {
  startSwitch(
    {
      kind: 'project',
      project: null,
      // This window's header strip, not the workspace's project map: in `PerProject` window
      // mode the map holds every project and the strip holds one. See `keys/target.ts`.
      live: () => windowProjectsOf(useWorkspace.getState().boot),
      order: () => useWorkspace.getState().mru,
      remember: (order) =>
        useWorkspace.getState().rememberMru(order as readonly ProjectId[]),
      activate: (id) => void useWorkspace.getState().activateProject(id as ProjectId),
      empty: '[cide] project switcher: this window holds fewer than two projects',
    },
    step,
  )
}

/** `tab.switcher.next` / `.prev` — Ctrl+Tab by default, and the palette. */
export function startTabSwitch(step: 1 | -1): void {
  const boot = useWorkspace.getState().boot
  const project = activeProjectIdOf(boot)
  if (boot === null || boot.role.kind !== 'shell' || project === null) {
    // A detached pane or tab window, or a shell with no project. The default binding carries
    // `shellWindow` so the chord is not even taken there, but a `Command::when` never gates the
    // keyboard and the palette is reachable from anywhere — so the fact is re-checked here, as
    // every handler in `keys/dispatch.ts` re-checks its own.
    void diag.log('[cide] tab switcher: this window has no tab strip')
    return
  }
  startSwitch(
    {
      kind: 'tab',
      project,
      live: () => tabIdsOf(project),
      order: () => useWorkspace.getState().tabMru[project] ?? [],
      remember: (order) =>
        useWorkspace.getState().rememberTabMru(project, order as readonly TabId[]),
      activate: (id) => revealTab(project, id as TabId),
      empty: '[cide] tab switcher: this project holds fewer than two tabs',
    },
    step,
  )
}

/** The active project's tabs, in strip order. `[]` when the project has gone. */
function tabIdsOf(project: ProjectId): readonly TabId[] {
  const open = useWorkspace.getState().boot?.workspace.projects[project]
  return open === undefined ? [] : open.tabs.map((tab) => tab.id)
}

/**
 * Go to a tab, through `revealPane` rather than `tab_activate`.
 *
 * `ws.activateTab` alone is what `tab.next` / `tab.prev` and a click on the strip do, and it is
 * half the gesture: the tab becomes visible and the keyboard is left in whatever the user was
 * typing in — which, since `TabContent` hides an inactive tab with `visibility: hidden`, is an
 * element the browser has just blurred. `revealPane` activates the tab *and* moves the domain's
 * focus and the caret into that tab's focused pane, in the one order that works
 * (`editor/revealPane.ts` argues it at length). For a terminal or a Claude pane that is the
 * whole difference between switching tabs and arriving somewhere you can type.
 *
 * The reason it answers with is logged rather than shown: the user asked to switch tabs and the
 * tab did switch, so a toast would be reporting a failure that did not happen.
 */
function revealTab(project: ProjectId, tab: TabId): void {
  const found = useWorkspace
    .getState()
    .boot?.workspace.projects[project]?.tabs.find((t) => t.id === tab)
  if (found === undefined) {
    void diag.log('[cide] tab switcher: that tab is no longer open')
    return
  }
  void revealPane(project, found.tree.focused).then((refusal) => {
    if (refusal !== null) void diag.log(`[cide] tab switcher: ${refusal}`)
  })
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

/*
 * There is deliberately no `useSwitcherSelection` here any more.
 *
 * It existed, it was exported, and **nothing called it** — `ProjectSwitcher.tsx` computed
 * `selection(walk)` itself, because it needs `walk.order` for the rows anyway and a second
 * subscription for one derived id would re-render the popup on its own state. Harmless, and the
 * sixteenth thing this project has found that was built and reachable from nothing; deleted
 * rather than left as a plausible-looking hook somebody would eventually wire up beside the one
 * that works.
 */
