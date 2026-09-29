/**
 * Which change the *Apply…* dialog is open on, or `null`.
 *
 * Its own module, holding nothing but the store, because the change page (`SpecTab.tsx`) opens
 * the dialog too — and that page exports a view `check:openspec-render` SSR-bundles under node,
 * where the dialog's own imports (the portal, the IPC client's session calls) have no business
 * being. `ProposeForm` is split out of `ProposeDialog` for the same reason.
 */
import { create } from 'zustand'

interface ApplyStore {
  change: string | null
  open: (change: string) => void
  close: () => void
}

export const useApplyDialog = create<ApplyStore>((set) => ({
  change: null,
  open: (change) => set({ change }),
  close: () => set({ change: null }),
}))
