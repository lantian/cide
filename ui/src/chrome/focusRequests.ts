/**
 * "The command opened a panel; now put the caret in its box."
 *
 * # The gap this closes
 *
 * A command that reveals a panel and stops there is half a command. `sidebar.search` opens the
 * search sidebar and, without this, leaves the caret wherever it was — usually a terminal — so
 * the user reaches for a keyboard shortcut and then has to finish the gesture with the mouse.
 * That is the same complaint, in a milder key, as the one this whole round is about: a control
 * that is reachable but does not do the thing it names. `git.commit` is the other case, and
 * `file.reveal` is the precedent for the ordering — reveal first, act second.
 *
 * # Why a store, and not `element.focus()` in the dispatcher
 *
 * Two constraints, and they rule out the two obvious mechanisms between them:
 *
 *   * **The panel is usually not mounted yet.** The dispatcher runs synchronously inside the
 *     key gate's window listener; `setView('search')` only mounts the panel on the *next* React
 *     render. A `document.querySelector(…).focus()` in the dispatcher finds nothing on the
 *     common path, which is every press made while the sidebar was on Files or shut.
 *   * **And sometimes it already is.** When the panel is on screen, revealing it mounts
 *     nothing, so an `autoFocus` on the input — which only fires on mount — silently does
 *     nothing on the second press. A user who pressed the chord and saw the panel not respond
 *     concludes the binding is broken.
 *
 * So the request is *parked* here and consumed by whichever render sees it first, mount or
 * re-render. It is the same handshake `sidebar/treeStore.ts` uses for `revealTo`, and for the
 * same reason.
 *
 * # Consumed, not merely observed
 *
 * The consumer calls [`clearFocusRequest`] once it has focused. That is what stops a stale
 * request from firing on an unrelated mount later — the panel unmounts every time the user
 * clicks another icon in the activity rail, and a flag that survived would mean that merely
 * *looking* at the Git panel snatched the caret into the commit message box. A pulse counter
 * would have exactly that bug; a consumed flag does not.
 *
 * No TTL, unlike `editor/revealRequest.ts`. Every caller here reveals the panel in the same
 * tick, so the render that consumes the flag is the very next one; the only way a request
 * outlives its gesture is a window with no sidebar at all, and those callers return before
 * asking.
 *
 * Transient window chrome, so a store of its own rather than anything in `store/workspace.ts`
 * — that one mirrors Rust-owned durable state and this must never be persisted. It is also
 * per-window by construction, which is right: each Tauri window has its own caret.
 */
import { create } from 'zustand'

/**
 * The inputs a command may ask for by name.
 *
 * A closed union rather than a free string: the whole value of this indirection is that the
 * dispatcher names a surface it cannot import, and a typo in a free-form key would be a
 * request nobody ever answers — which is the failure mode, not a variant of it.
 */
export type FocusSurface =
  /** The search panel's query box, for `sidebar.search`. */
  | 'search'
  /** The Git panel's commit message textarea, for `git.commit`. */
  | 'commitMessage'

interface FocusRequestStore {
  /** The surfaces with an unanswered request. Empty almost always. */
  pending: readonly FocusSurface[]
  request: (surface: FocusSurface) => void
  clear: (surface: FocusSurface) => void
}

const useFocusRequests = create<FocusRequestStore>((set, get) => ({
  pending: [],

  request(surface) {
    // Idempotent. A second request for a surface nobody has consumed yet is the same request:
    // the panel has not rendered since the first one, so there is nothing new to say.
    if (get().pending.includes(surface)) return
    set({ pending: [...get().pending, surface] })
  },

  clear(surface) {
    if (!get().pending.includes(surface)) return
    set({ pending: get().pending.filter((entry) => entry !== surface) })
  },
}))

/**
 * Ask for the caret, from outside React.
 *
 * Called by `keys/dispatch.ts`, which runs in a window listener and has no component to be in.
 */
export function requestFocus(surface: FocusSurface): void {
  useFocusRequests.getState().request(surface)
}

/** Say the request has been answered. Safe to call when there was none. */
export function clearFocusRequest(surface: FocusSurface): void {
  useFocusRequests.getState().clear(surface)
}

/**
 * Whether `surface` has an unanswered request, subscribed.
 *
 * Selecting a boolean rather than the array: zustand compares selector results with
 * `Object.is`, so a component watching the array would re-render whenever any *other* surface
 * was asked for.
 */
export function useFocusRequested(surface: FocusSurface): boolean {
  return useFocusRequests((state) => state.pending.includes(surface))
}
