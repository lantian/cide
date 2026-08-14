/**
 * Which overlay is open. **The** answer to that question — there is no second one.
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
 *
 * # Why the emphasis
 *
 * This store shipped with nothing reading it. `App` kept its own `useState`, `OverlayHost`
 * rendered from *this* one, and the key gate wrote to this one — so Ctrl+P set a flag that
 * `App` never saw, `App` never mounted the host, and the picker could not be opened by any
 * gesture at all. Two sources of truth for one boolean is not a style question here; it was
 * the entire feature being unreachable.
 *
 * The exports below exist so `App` can drop its copy: [`useOverlayOpen`] for rendering and
 * for the `overlayOpen` key context, [`showOverlay`] / [`closeOverlay`] for the callbacks
 * that used to call `setOverlay`. Nothing else is needed, and nothing else should be added
 * — a third writer is how this happens again.
 */
import { create } from 'zustand'

/**
 * `branches` is the status bar's branch popup (`chrome/BranchSelector.tsx`).
 *
 * It is an overlay rather than a child of the widget so it has exactly one instance per
 * window — and so the command palette can open it without the status bar being involved at
 * all. A popup owned by the widget would be unreachable in any window whose bar does not
 * mount it, which is the "control wired to nothing" shape this app keeps producing.
 */
export type OverlayKind = 'files' | 'commands' | 'branches' | 'structure' | 'symbols'

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

/**
 * Subscribe to which overlay is open. What `App` renders from.
 *
 * A one-line hook rather than asking every caller to write the selector: `useOverlays((s) =>
 * s.open)` written in two places is two places to accidentally write `useOverlays()` instead
 * and re-render the shell on every action identity change.
 */
export function useOverlayOpen(): OverlayKind | null {
  return useOverlays((s) => s.open)
}

/**
 * Open an overlay from outside React — a dispatched command, a menu item.
 *
 * `show`, not `toggle`: a caller that names the overlay it wants means it. Toggling belongs
 * to the key gate, where the same chord has to be able to undo itself.
 */
export function showOverlay(kind: OverlayKind): void {
  useOverlays.getState().show(kind)
}

/** Close whatever is open, from outside React. The picker's "I accepted a row" path. */
export function closeOverlay(): void {
  useOverlays.getState().close()
}
