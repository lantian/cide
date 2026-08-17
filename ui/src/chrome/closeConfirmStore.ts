/**
 * The pending close confirmations, and how they are answered — one dialog at a time.
 *
 * A store rather than local state in `App.tsx` because the gesture and the dialog are in
 * different places: the close is asked for by a `×` in the tab strip, by a header menu, by
 * a keybinding, or by the window's own close button, and every one of them has to end at
 * the same dialog. Threading a callback from each of those to a `useState` in the shell is
 * the version of this that works until the fourth call site.
 *
 * Deliberately **not** part of `store/workspace.ts`: that store is a mirror of Rust-owned
 * durable state and holds nothing local. This is transient chrome — it does not survive a
 * reload and must never be persisted — which is the same category as which overlay is open.
 */
import { create } from 'zustand'
import type { SessionSummary, UnsavedTab } from '@/ipc/client'
import { advanceClose, parkClose, type CloseScope } from './closeConfirmModel'

export interface PendingClose {
  scope: CloseScope
  unsaved: UnsavedTab[]
  sessions: SessionSummary[]
  /**
   * Re-issue the close the user just confirmed, with `force`.
   *
   * A closure rather than the ids, because the four scopes end in four different commands
   * and a discriminated union of their arguments would be reconstructed at the one place
   * that has all of them anyway — the call site that was refused.
   */
  proceed: () => Promise<void>
}

interface CloseConfirmStore {
  /** Null whenever no confirmation is on screen, which is almost always. */
  pending: PendingClose | null
  /**
   * Refusals waiting their turn. Empty for every single close, which is almost all of them.
   *
   * Not shown, and never more than one dialog is: this is the tail behind [`pending`].
   */
  queued: PendingClose[]
  /**
   * Park a refusal.
   *
   * One dialog is on screen at a time; a second request **queues** behind the first rather
   * than replacing it. Replacing was right while every close was one tab — the two requests
   * could only be the same gesture asked twice. The tab and project context menus broke that:
   * "Close others" fires one `closeTab` per tab, they run concurrently, and two of them can
   * be refused for unsaved changes in the same tick. Under the old rule the user answered for
   * one file and the other tab silently stayed open, which is the exact shape of bug —
   * a gesture that half-works and says nothing — this whole surface exists to remove.
   */
  request: (pending: PendingClose) => void
  /**
   * The user went back. Nothing was closed and nothing was written.
   *
   * Drops the queue as well as the dialog. Cancel answers the *gesture*, not one file of it:
   * having said no to closing the first of four tabs, being asked three more times is how a
   * user learns to hit Escape without reading.
   */
  dismiss: () => void
  /**
   * The user chose to proceed.
   *
   * Clears `pending` *before* awaiting, so the dialog is gone while the close runs and a
   * second Enter cannot fire `proceed` twice. A failure re-throws — the caller's own error
   * path reports it — but the dialog does not come back, because the answer was given.
   *
   * The next queued refusal takes its place in the same update, so a bulk close asks once per
   * thing at stake instead of once in total.
   */
  confirm: () => Promise<void>
}

export const useCloseConfirm = create<CloseConfirmStore>((set, get) => ({
  pending: null,
  queued: [],
  request: (pending) => set((state) => parkClose(state.pending, state.queued, pending)),
  dismiss: () => set({ pending: null, queued: [] }),
  confirm: async () => {
    const pending = get().pending
    if (!pending) return
    // The whole transition in one update, and before the await: `proceed` re-issues the close
    // with `force`, which can itself be refused for something else and call `request` again.
    // Leaving `pending` set across that would put the new refusal at the back of a queue
    // behind a dialog that has already been answered.
    set((state) => advanceClose(state.queued))
    await pending.proceed()
  },
}))

/** Park a refusal from outside React. Used by `store/workspace.ts`'s close paths. */
export function requestCloseConfirm(pending: PendingClose): void {
  useCloseConfirm.getState().request(pending)
}
