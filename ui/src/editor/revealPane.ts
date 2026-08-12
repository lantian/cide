/**
 * Take the user to the pane a mention just landed in.
 *
 * The half of *send lines to Claude* that moves things. `revealTarget.ts` decides what has to
 * change and is unit-tested under node; this runs it, in an order that is the whole of the
 * remaining difficulty.
 *
 * # The order, and why it is that order
 *
 * 1. **The tab, then the maximize, then the domain's focus.** All three are workspace
 *    mutations, so they are one shared truth every window agrees on — which is what makes a
 *    reveal *into another window* possible at all: this webview cannot touch that one's React
 *    tree, but it can move the domain and let `cide://workspace-changed` carry it over.
 * 2. **The raise.** Last, so the window that comes forward is already showing the right tab.
 *    Raising first would put a window on screen displaying whatever the user last left in it
 *    and then switch tabs under their eyes a frame later.
 * 3. **The keyboard, and the scrollback.** Only when the pane is in *this* window, and only
 *    after React has committed the tab switch: `TabContent` keeps every tab mounted and hides
 *    the inactive ones, and the browser refuses focus to an element under `visibility: hidden`
 *    — so focusing before the commit is a call that silently does nothing.
 *
 * # Why the domain's focus moves and not only the scroll
 *
 * Because the two must not disagree. `tree.focused` is what the key gate derives
 * `claudePaneFocused` from and what `useMentionTarget` reads to pick the *next* mention's
 * destination; the DOM's `activeElement` is where the next keystroke goes. Moving one without
 * the other invents a state this app has never had — every other route to a focused pane, a
 * click included, moves both — and the first symptom would be a second ⌥⏎ aiming somewhere
 * the user was not looking.
 *
 * Setting the domain's focus *before* the terminal's also costs one round trip less than the
 * other order: `SplitTree` raises focus from an `onFocusCapture` unless the tree already names
 * the pane, so by the time `term.focus()` fires its event there is nothing left for it to do.
 */
import { peekHost } from '@/layout/paneHosts'
import { sendFocus, type PaneId, type ProjectId } from '@/ipc/client'
import { useWorkspace } from '@/store/workspace'
import { planReveal } from './revealTarget'

/** Whatever was thrown, as a clause that can be dropped into a sentence. */
function why(error: unknown): string {
  if (error instanceof Error) return error.message
  if (typeof error === 'string') return error
  if (error !== null && typeof error === 'object') {
    const message = (error as { message?: unknown }).message
    if (typeof message === 'string' && message.length > 0) return message
  }
  return String(error)
}

/**
 * Resolve once React has committed and the browser has laid out against it.
 *
 * Two frames, not one, and `store/workspace.ts` has the same helper for the same reason: the
 * first callback runs before the commit that the state update schedules has been painted.
 */
function committed(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => requestAnimationFrame(() => resolve()))
  })
}

/**
 * Bring `pane` into view and put the keyboard in it.
 *
 * Answers `null` when the user is now looking at the pane, and otherwise **the reason they
 * are not** — a clause, not a sentence, so the caller can say what it was trying to do. It
 * never throws: a reveal that fails after a send that succeeded is not a failed command, and
 * reporting it as one would put a red toast in front of a mention that did arrive.
 */
export async function revealPane(project: ProjectId, pane: PaneId): Promise<string | null> {
  const ws = useWorkspace.getState()
  const boot = ws.boot
  // Only before the first `app.getBootstrap` resolves, which is before any editor exists to
  // send from. Named anyway: `boot` is nullable and a silent `return null` here would be the
  // one path that claims success without doing anything.
  if (boot === null) return 'this window has not finished loading'

  const plan = planReveal(boot, project, pane)
  if (plan.blocked !== null) return plan.blocked

  if (plan.tab !== null) {
    const tab = plan.tab
    try {
      if (plan.activateTab) await ws.activateTab(project, tab)
      if (plan.clearMaximize) await ws.maximizePane(project, tab, null)
      if (plan.focusPane) await ws.focusPane(project, tab, pane)
    } catch (error) {
      return why(error)
    }
  }

  if (!plan.here) {
    // The one thing a webview cannot do. Skipped when the pane is in this window, because
    // this window is the one the gesture was just made in and is therefore already in front:
    // the round trip would buy nothing, and asking a compositor to focus the window that
    // already has focus is the kind of call that flashes a taskbar entry on some of them.
    try {
      const label = await sendFocus.revealPane(project, pane)
      if (label === null) return 'no window is showing it'
    } catch (error) {
      return `its window could not be brought forward — ${why(error)}`
    }
    /*
     * **What the cross-window case does and does not deliver, stated rather than implied.**
     *
     * It delivers the visible half in full: the right window is in front, showing the right
     * tab, with the focus ring on the right pane — all of it from the domain, which both
     * windows share. It does not deliver the caret. `term.focus()` below is a call into
     * *this* realm's DOM and the pane is in another one, and there is no channel that says
     * "put your keyboard in this element" to a window that is not this one.
     *
     * Left alone rather than papered over. The alternative was to make every window move its
     * DOM focus whenever `tree.focused` changes, which would be a new global rule — a pane
     * focused by a click in one window would start stealing the caret in another — invented
     * to serve one gesture. The user arrives looking at their prompt and one click away from
     * typing into it; that is the smaller gap.
     */
    return null
  }

  await committed()

  const terminal = peekHost(pane)?.terminal
  if (terminal === undefined) {
    // The pane is on screen — the tab switch and the focus ring both happened — but its
    // terminal is not resident: an evicted host that has not been rebuilt yet, or a pane
    // whose slot has not mounted. Reported as success rather than as a reason, because the
    // user *is* looking at the pane, which is what they asked for; only the caret is
    // elsewhere, and their next click fixes that.
    return null
  }
  // The prompt is at the bottom, and a user who had scrolled up through the transcript would
  // otherwise be shown the pane with their mention off screen below the fold — the same
  // "nothing happened" in miniature. xterm only follows new output when the viewport is
  // already at the bottom, so this is not something the write itself would have done.
  terminal.term.scrollToBottom()
  terminal.term.focus()
  return null
}
