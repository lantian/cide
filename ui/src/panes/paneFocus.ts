/**
 * How a command puts the keyboard in a pane.
 *
 * `pane.navigate.*` moves the domain's focus — `pane_focus` writes `tree.focused`, which is
 * the ring, the context flags and the mention target — and until this module existed that was
 * *all* it moved. The report was exact: "moves focus (the highlight) but not the real focus —
 * after this hotkey input stays in previous panel". `editor/revealPane.ts`'s header states
 * the rule that makes that a bug and not a choice: the domain's focus and the DOM's
 * `activeElement` must not disagree, because every other route to a focused pane — a click
 * included — moves both, and the first symptom of splitting them is a keystroke landing
 * somewhere the user is not looking.
 *
 * `keys/dispatch.ts` runs outside React and cannot call into a component, so the reach goes
 * through a module, exactly the way `paneRestart.ts` reaches a pane's restart and
 * `editor/revealRequest.ts` reaches a buffer's caret. Two kinds of pane, two routes:
 *
 * * **Terminals need no registration.** Their DOM lives outside React already
 *   (`layout/paneHosts.ts`), so [`focusPaneDom`] asks the host registry directly. This is
 *   the whole of the reported case — claude and bash panes.
 * * **Document panes publish.** A CodeMirror view is only reachable from inside its mount,
 *   so the pane hands a `focus` closure in for as long as it is mounted — the
 *   `onSaveHandle`/`onScrollHandle` shape, one seam further out.
 *
 * A pane that is neither — a document pane that has not registered (today: the merge
 * resolver, the diff surfaces, an image) — answers `false`, and the caller logs rather than
 * pretends: the ring still moves, the caret stays, and the next click fixes it. That is the
 * gap `revealPane` already ships for a terminal that is not resident, made visible in the
 * diagnostic log instead of silent.
 */
import { peekHost } from '@/layout/paneHosts'

const receivers = new Map<string, () => void>()

/**
 * Publish a pane's "put the keyboard in me". Returns the teardown.
 *
 * The teardown checks ownership before it deletes, for the reason `paneRestart.ts` states
 * about the same shape: React can mount a replacement before it runs the previous mount's
 * cleanup, and a stale teardown that deleted unconditionally would leave the live pane
 * unfocusable by keyboard.
 */
export function registerPaneFocus(paneId: string, focus: () => void): () => void {
  receivers.set(paneId, focus)
  return () => {
    if (receivers.get(paneId) === focus) receivers.delete(paneId)
  }
}

/**
 * Move the DOM's focus into `pane`, if this window can.
 *
 * Answers whether anything took the keyboard. `false` is not an error — the pane may be in
 * another window, parked, or a document kind that has not published — but the caller should
 * say so in the diagnostic log, because a silent `false` here is the reported bug back again
 * for whichever pane kind stopped registering.
 */
export function focusPaneDom(paneId: string): boolean {
  const receiver = receivers.get(paneId)
  if (receiver !== undefined) {
    receiver()
    return true
  }
  // No scroll, unlike `revealPane`'s terminal tail: a navigate is a focus move between
  // panes the user is already looking at, and yanking the scrollback to the bottom would
  // move text they may have been reading. Focus alone is the half that was missing.
  const terminal = peekHost(paneId)?.terminal
  if (terminal !== undefined) {
    terminal.term.focus()
    return true
  }
  return false
}
