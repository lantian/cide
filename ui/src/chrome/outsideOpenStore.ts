/**
 * The out-of-project open waiting on an answer — one at a time, in whichever window asked.
 *
 * A store rather than local state in `App.tsx` for the reason `closeConfirmStore.ts` is one, and
 * one reason more:
 *
 * * The gesture is a native `mousedown` listener attached by `terminal/pathLinks.ts` to an
 *   xterm host that lives outside React entirely, and the answer has to come back to a
 *   component. Threading a callback down through `paneHosts` to a DOM listener is the version
 *   of this that works until the second window.
 * * **Both window kinds have to draw it.** `App.tsx` renders a shell window *or* a
 *   `pane:<uuid>` window, and the detached branch returns early. A dialog wired only into the
 *   shell tree means a ctrl+click in a torn-out pane asks nobody and opens nothing — which is
 *   exactly the defect Lane A found in `restorePlan`, in new clothes. `OutsideOpenGate` is
 *   rendered in both branches, next to `<Failures/>`, and this store is what it reads.
 *
 * Deliberately **not** part of `store/workspace.ts`: that store mirrors Rust-owned durable state
 * and holds nothing local. An approval is transient by design — see the "no don't-ask-again"
 * note in `terminal/outsideOpen.ts` — and must never be persisted.
 *
 * # One at a time, and no queue
 *
 * `OverlayCard` draws a scrim over the whole window, so while a question is up there is no
 * terminal to ctrl+click and a second request cannot be made by hand. A request arriving anyway
 * — a stray programmatic call — is **dropped**, not queued and not allowed to replace: replacing
 * would swap the path under a user who is mid-read, which is the one thing a dialog whose entire
 * job is *read this path* must never do. `closeConfirmStore` queues because "Close others" genuinely
 * fires one refusal per tab; nothing here fires in bulk.
 */
import { create } from 'zustand'
import type { OutsideAsk } from '@/terminal/outsideOpen'

export interface PendingOutsideOpen {
  ask: OutsideAsk
  /**
   * Re-issue the open with the approval attached.
   *
   * A closure rather than the arguments, because the retry needs the project, the path, the
   * caret the click carried and the hydrate that follows — all of which the call site that was
   * refused is already holding.
   */
  proceed: () => void
}

interface OutsideOpenStore {
  /** Null whenever no question is on screen, which is almost always. */
  pending: PendingOutsideOpen | null
  request: (pending: PendingOutsideOpen) => void
  /** The user backed out. Nothing was read and nothing was opened. */
  dismiss: () => void
  /** The user said yes. Cleared *before* `proceed`, so a second Enter cannot fire it twice. */
  confirm: () => void
}

export const useOutsideOpen = create<OutsideOpenStore>((set, get) => ({
  pending: null,
  request: (pending) => set((state) => (state.pending === null ? { pending } : state)),
  dismiss: () => set({ pending: null }),
  confirm: () => {
    const pending = get().pending
    if (pending === null) return
    set({ pending: null })
    pending.proceed()
  },
}))

/** Ask, from outside React. `App.tsx`'s terminal-open handler is the only caller. */
export function requestOutsideOpen(pending: PendingOutsideOpen): void {
  useOutsideOpen.getState().request(pending)
}
