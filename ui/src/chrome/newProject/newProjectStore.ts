/**
 * Whether the New project wizard is up, in this window. (M97)
 *
 * A store rather than state in `App.tsx` for `pushStore.ts`'s reason: the gesture is made from
 * `keys/dispatch.ts` and from the header's project menu, and neither is inside the component that
 * draws the wizard. The wizard's form lives in the component and is reset on every open — a
 * half-filled wizard reappearing a day later with somebody else's path in it is not a feature.
 *
 * Not in `overlays/store.ts` either: `OverlayHost` mounts only while a project is open, and the
 * empty shell — no project yet — is exactly where a first project is made.
 */
import { create } from 'zustand'

interface NewProjectStore {
  open: boolean
  show: () => void
  close: () => void
}

export const useNewProject = create<NewProjectStore>((set) => ({
  open: false,
  show: () => set({ open: true }),
  close: () => set({ open: false }),
}))

/** Raise the wizard, from outside React. `keys/dispatch.ts` and `ProjectMenu.tsx` call this. */
export function requestNewProject(): void {
  useNewProject.getState().show()
}
