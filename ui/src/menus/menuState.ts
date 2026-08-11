/**
 * Whether *a* context menu is open, anywhere in this window.
 *
 * One counter rather than a boolean: two surfaces can be mid-teardown and mid-open in the
 * same commit (right-clicking a tree row while a tab-strip menu is up), and a boolean would
 * be left `false` by whichever effect cleaned up second.
 *
 * This exists for one consumer — the key gate's `when` context. `gate.ts` is a **window
 * capture** listener, so it sees a keystroke before the open menu does; every default
 * binding is a modified chord and none of the menu's keys (Escape, arrows, Enter, Tab,
 * Home/End) is bound, so nothing collides today. The moment a user binds a bare key it would,
 * and the fix is a `contextMenuOpen` flag in `App.tsx`'s `keyContext` guarding those bindings
 * — which is why the flag is published here rather than kept private to the hook.
 */
import { create } from 'zustand'

interface MenuState {
  /** How many menus are mounted. */
  open: number
  opened: () => void
  closed: () => void
}

const useMenuState = create<MenuState>((set) => ({
  open: 0,
  opened: () => set((s) => ({ open: s.open + 1 })),
  // Floored, so a double-cleanup under StrictMode cannot drive the count negative and leave
  // the flag stuck on.
  closed: () => set((s) => ({ open: Math.max(0, s.open - 1) })),
}))

/** Subscribe. For `App.tsx`'s `keyContext`. */
export function useContextMenuOpen(): boolean {
  return useMenuState((s) => s.open > 0)
}

/** Read once, outside React. */
export function contextMenuOpen(): boolean {
  return useMenuState.getState().open > 0
}

/** Internal: `useContextMenu` reports its own lifetime through these. */
export const menuLifetime = {
  opened: () => useMenuState.getState().opened(),
  closed: () => useMenuState.getState().closed(),
}
