/**
 * The pull waiting on *merge or rebase* — one question per gesture, in whichever window asked.
 *
 * A store rather than local state in `App.tsx` for `outsideOpenStore.ts`'s reasons, both of
 * which apply here unchanged: the gesture is made from `keys/dispatch.ts`, which is outside
 * React's tree entirely, and **both window kinds have to draw it** — `useKeyGate` is installed
 * before `App.tsx`'s `pane:<uuid>` early return, and `git.pull`'s `when` clause is
 * `projectOpen` rather than `shellWindow && projectOpen`, so Ctrl+T is live in a detached pane
 * window. A dialog wired only into the shell tree would leave that keystroke asking nobody.
 *
 * Deliberately **not** part of `store/workspace.ts`: that store mirrors Rust-owned durable
 * state and holds nothing local. The answer here is transient by design — the durable half is
 * the `pull.rebase` line Rust writes when the box is ticked.
 *
 * # Ask once, apply to all — and why this does not queue
 *
 * `git.pull` fans out over every repository in the project, so two can diverge at once.
 * `closeConfirmStore` queues because each of *its* refusals names **different work at stake** —
 * this file, then that one — and the user genuinely has to judge each. Here the decision is a
 * **policy**: how do I reconcile a divergence. It is literally what one `pull.rebase` key
 * expresses, and asking it once per repository would be asking the same question with a
 * different noun in it.
 *
 * The precedent is inside `closeConfirmModel.ts` itself, whose `tabs` scope exists *because
 * queueing was wrong*: three gestures used to fire one refusal per dirty tab and show a dialog
 * naming a different file each time, for a gesture the user made once. Ctrl+T is that gesture.
 * `pullReport`'s header states the doctrine outright — *one gesture deserves one answer*.
 *
 * So `dispatch.ts` collects every repository that asked into a single `PullStrategyAsk` before
 * requesting, and this store holds exactly one at a time. A second request arriving while one is
 * up is **dropped**, not queued and not allowed to replace: `OverlayCard` scrims the window, so
 * the only way to make one by hand is another Ctrl+T — the same gesture repeated — and
 * replacing would swap the question under somebody mid-read.
 */
import { create } from 'zustand'
import type { PullStrategy, PullStrategyAsk, RepoDivergence } from './pullStrategyModel'

export interface PendingPullStrategy {
  ask: PullStrategyAsk
  /** Every repository the answer covers, so the label can name them and the retry can re-issue. */
  repos: readonly RepoDivergence[]
  /**
   * Re-issue the pull with the answer attached.
   *
   * A closure rather than the arguments, because the retry needs the project, the repository
   * list and the fetch/report machinery — all of which the call site that was refused is
   * already holding.
   */
  proceed: (strategy: PullStrategy, remember: boolean) => void
}

interface PullStrategyStore {
  /** Null whenever no question is on screen, which is almost always. */
  pending: PendingPullStrategy | null
  request: (pending: PendingPullStrategy) => void
  /** The user backed out. Nothing was integrated and nothing was written. */
  dismiss: () => void
  /** Cleared *before* `proceed`, so a second Enter cannot fire it twice. */
  answer: (strategy: PullStrategy, remember: boolean) => void
}

export const usePullStrategy = create<PullStrategyStore>((set, get) => ({
  pending: null,
  request: (pending) => set((state) => (state.pending === null ? { pending } : state)),
  dismiss: () => set({ pending: null }),
  answer: (strategy, remember) => {
    const pending = get().pending
    if (pending === null) return
    set({ pending: null })
    pending.proceed(strategy, remember)
  },
}))

/** Ask, from outside React. `keys/dispatch.ts` and `chrome/BranchSelector.tsx` are the callers. */
export function requestPullStrategy(pending: PendingPullStrategy): void {
  usePullStrategy.getState().request(pending)
}
