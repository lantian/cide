/**
 * Which terminal panes in this window have their find bar up.
 *
 * # Why a store and not a prop
 *
 * The gesture starts in two places that are outside the React tree entirely: `keys/dispatch.ts`,
 * which runs from a window key listener and from the command palette, and the composed handler
 * in `terminal/xterm.ts`, which xterm calls for a keydown on its own textarea. Neither has a
 * component to call into. That is the same predicament `overlays/usagesStore.ts` and
 * `keys/recorderStore.ts` are in and it has the same answer here.
 *
 * The register-a-handle shape (`panes/paneRestart.ts`) was the alternative and it loses for this
 * one: a restart is an *imperative* thing only the pane can do, so the pane has to publish a
 * function. Opening a find bar is a piece of **state** — a bar is up or it is not — and a
 * handle-based version would need the pane to publish a setter that only ever writes state the
 * pane itself owns, which is the same store with an extra hop and one more thing to unregister.
 *
 * # Why the value is a counter and not `true`
 *
 * Pressing Ctrl+F while the bar is already open has to *do something* — re-focus the field and
 * select what is in it, so the chord that opened the bar retypes the query rather than appearing
 * dead. That is a repeatable event, and a boolean cannot carry a repeat: the second `true` is
 * the same `true`, no subscriber is notified, and the second press does nothing. The editor's
 * find bar has exactly this bug in its history — `openSearchPanel`'s already-open branch is
 * where `@codemirror/search` puts the re-focus, and `editor/find.ts`'s header records what it
 * cost to leave it out.
 *
 * So the value is the number of times this pane has been asked, and `TerminalFindBar` re-focuses
 * whenever it changes. `undefined` — the key absent — is closed.
 *
 * # Per window, and that is the right scope
 *
 * A module-level store is per JavaScript realm, and a detached pane window is a realm of its own
 * (ADR 0001). A pane lives in exactly one window at a time — a "mirror" is a second pane with
 * the same session, not the same pane — so there is never a bar to keep in sync across windows,
 * and nothing here needs to reach `cide-core::workspace`. A find bar is transient gesture state,
 * which is one of the exactly two things the webview is allowed to own.
 */
import { create } from 'zustand'

interface TerminalFindStore {
  /**
   * Pane id → how many times its bar has been asked to open. Absent means the bar is closed.
   *
   * A plain record rather than a `Map` so a component can subscribe to one pane's entry with
   * `useTerminalFind((s) => s.open[paneId])`: zustand compares selector results with `Object.is`,
   * and a number (or `undefined`) compares correctly where a fresh object would re-render on
   * every notification for ever.
   */
  open: Readonly<Record<string, number>>
  /** Open this pane's find bar, or — if it is already open — ask it to re-focus its field. */
  openFind: (paneId: string) => void
  /** Close it. Safe to call for a pane that has no bar; nothing is notified if nothing changes. */
  closeFind: (paneId: string) => void
}

export const useTerminalFind = create<TerminalFindStore>((set) => ({
  open: {},

  openFind: (paneId) =>
    set((s) => ({ open: { ...s.open, [paneId]: (s.open[paneId] ?? 0) + 1 } })),

  closeFind: (paneId) =>
    set((s) => {
      // Checked before rebuilding, so closing a pane that has no bar — which is what every
      // unmount of every terminal pane in the window does — is not a state write and not a
      // render. Without this, one pane closing re-renders the find bar of every other.
      if (s.open[paneId] === undefined) return s
      const next = { ...s.open }
      delete next[paneId]
      return { open: next }
    }),
}))

/**
 * Open (or re-focus) a pane's find bar, from outside React.
 *
 * The seam Ctrl+F (`terminal/xterm.ts`), the palette and any user binding (`keys/dispatch.ts`)
 * and the pane's context menu (`panes/TerminalPane.tsx`) all call. Named rather than left as
 * `useTerminalFind.getState().openFind` at each call site so that grepping for the feature finds
 * every entry point.
 */
export function openTerminalFind(paneId: string): void {
  useTerminalFind.getState().openFind(paneId)
}

/** Close a pane's find bar. Escape, the ✕ button, and the bar's own unmount. */
export function closeTerminalFind(paneId: string): void {
  useTerminalFind.getState().closeFind(paneId)
}
