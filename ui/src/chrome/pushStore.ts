/**
 * The push waiting to be confirmed — one question per gesture, in whichever window asked.
 *
 * A store rather than local state in `App.tsx` for `pullStrategyStore.ts`'s two reasons, both of
 * which apply here unchanged: the gesture is made from `keys/dispatch.ts`, which is outside
 * React's tree entirely, and **both window kinds have to draw it** — `useKeyGate` is installed
 * before `App.tsx`'s `pane:<uuid>` early return, and `git.push`'s `when` clause is `projectOpen`
 * rather than `shellWindow && projectOpen`, so the command is live in a detached pane window. A
 * dialog wired only into the shell tree would leave that keystroke asking nobody.
 *
 * Deliberately **not** part of `store/workspace.ts`: that store mirrors Rust-owned durable state
 * and holds nothing local. Everything here is transient — the previews are a reading of the
 * repository taken when the dialog opened, and nothing about them is worth persisting.
 *
 * # Why this does not queue
 *
 * `pullStrategyStore.ts`'s argument, and it is the stronger one here. `git.push` names no
 * repository, so it acts on every one in the project; the dialog therefore *is* the whole
 * gesture rather than one repository's share of it, and there is nothing a second one could ask.
 * A request arriving while one is up is **dropped**, not queued and not allowed to replace:
 * `OverlayCard` scrims the window, so the only way to make one by hand is to repeat the gesture,
 * and replacing would swap the commit list under somebody mid-read — the one list in this app
 * whose whole purpose is being read before a button is pressed.
 */
import { create } from 'zustand'
import type { PushPreviewLike } from './pushModel'

export interface PendingPush {
  /** Named so the retry can re-issue without re-resolving the active project. */
  project: string
  /** Every repository in the project, blocked ones included — the dialog draws them all. */
  previews: readonly PushPreviewLike[]
  /**
   * Send it.
   *
   * A closure rather than the arguments, because the push needs the project, the repository list
   * and the report machinery — all of which the call site that opened this is already holding,
   * and one of which (`pushPass`) is where the fan-out, the aggregation and the failure path
   * live. `repos` is the ticked subset, in the order they were drawn.
   */
  proceed: (repos: readonly string[], force: boolean) => void
}

interface PushStore {
  /** Null whenever no push is waiting, which is almost always. */
  pending: PendingPush | null
  request: (pending: PendingPush) => void
  /** The user backed out. Nothing was sent. */
  dismiss: () => void
  /** Cleared *before* `proceed`, so a second Enter cannot push twice. */
  confirm: (repos: readonly string[], force: boolean) => void
}

export const usePush = create<PushStore>((set, get) => ({
  pending: null,
  request: (pending) => set((state) => (state.pending === null ? { pending } : state)),
  dismiss: () => set({ pending: null }),
  confirm: (repos, force) => {
    const pending = get().pending
    if (pending === null) return
    set({ pending: null })
    pending.proceed(repos, force)
  },
}))

/** Ask, from outside React. `keys/dispatch.ts` and `chrome/BranchSelector.tsx` are the callers. */
export function requestPush(pending: PendingPush): void {
  usePush.getState().request(pending)
}
