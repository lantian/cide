/**
 * Which overlay is open.
 *
 * Local, transient window state — the one category `store/workspace.ts` names as genuinely
 * owned by the webview ("which overlay is open, whether a splitter is mid-drag"). It must
 * not go to Rust: two windows each have their own picker, and a detached pane window opening
 * one has no business raising a modal over the shell.
 *
 * Kept in a store rather than `useState` in `App` because the key gate dispatches
 * `picker.files` from outside React's tree, and a zustand store can be written from there
 * with `getState()` without threading a callback through every component that installs the
 * gate.
 */
import { create } from 'zustand'

export type OverlayKind = 'files' | 'commands'

interface OverlayStore {
  open: OverlayKind | null
  show: (kind: OverlayKind) => void
  close: () => void
  /**
   * Open `kind`, or close it if it is already the one showing.
   *
   * Pressing Ctrl+P while the picker is open closes it — the same key toggling its own
   * surface is what every editor does. Pressing Ctrl+Shift+P while the *picker* is open
   * switches to the palette rather than closing, which falls out of comparing the kind.
   */
  toggle: (kind: OverlayKind) => void
}

export const useOverlays = create<OverlayStore>((set) => ({
  open: null,
  show: (kind) => set({ open: kind }),
  close: () => set({ open: null }),
  toggle: (kind) => set((state) => ({ open: state.open === kind ? null : kind })),
}))

/** True when any overlay is showing. The `overlayOpen` flag `when` clauses test. */
export function overlayOpen(): boolean {
  return useOverlays.getState().open !== null
}
