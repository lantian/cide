/**
 * The one pending close confirmation, and how it is answered.
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
import type { CloseScope } from './closeConfirm'

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
  /** Park a refusal. A second request replaces the first — one dialog, ever. */
  request: (pending: PendingClose) => void
  /** The user went back. Nothing was closed and nothing was written. */
  dismiss: () => void
  /**
   * The user chose to proceed.
   *
   * Clears `pending` *before* awaiting, so the dialog is gone while the close runs and a
   * second Enter cannot fire `proceed` twice. A failure re-throws — the caller's own error
   * path reports it — but the dialog does not come back, because the answer was given.
   */
  confirm: () => Promise<void>
}

export const useCloseConfirm = create<CloseConfirmStore>((set, get) => ({
  pending: null,
  request: (pending) => set({ pending }),
  dismiss: () => set({ pending: null }),
  confirm: async () => {
    const pending = get().pending
    if (!pending) return
    set({ pending: null })
    await pending.proceed()
  },
}))

/** Park a refusal from outside React. Used by `store/workspace.ts`'s close paths. */
export function requestCloseConfirm(pending: PendingClose): void {
  useCloseConfirm.getState().request(pending)
}
