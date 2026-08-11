/**
 * Unified or side-by-side, and where that answer is kept.
 *
 * The mode is a stored preference (`Settings.editor.diffView`), not pane state, because a
 * diff pane is opened and thrown away constantly — every file in the git panel is a new tab
 * — and a toggle that resets with the pane is a toggle nobody can stay in. Persisting it also
 * means the *first* diff of a session already opens the way the user left it, which is the
 * only version of "remembered" that is worth anything.
 *
 * # Why this is not `settings/useSettings.ts`
 *
 * That hook is the right route for every screen in the app and the wrong one for this pane.
 * `useSettings` reads `@/store/workspace`, which imports `@/layout/paneHosts`, which imports
 * xterm — and `ui/scripts/check-diff-render.mjs` SSR-bundles `GitDiffView` under node to prove
 * that what the pane highlights is what it stages. Pulling a terminal into that bundle breaks
 * the one check standing between a selection and the wrong lines being staged.
 * `GitDiffPane.tsx` already says this about `store/workspace` and subscribes to
 * `cide://workspace-changed` directly for the same reason; this module is that same trade,
 * factored out so both diff panes share one copy of the answer.
 *
 * So the dependency here is `@/ipc/client` and nothing else, which is exactly what the pane
 * already imports.
 *
 * # Why a module-level cache rather than a hook per pane
 *
 * Every open diff tab stays mounted (`TabContent` keeps them so switching is instant), so a
 * per-pane `settings.get()` would be one IPC round trip per tab on every mount. One cache, one
 * seeding call, one event subscription, and `useSyncExternalStore` fans it out.
 *
 * The snapshot is reference-stable between changes, which is not tidiness:
 * `useSyncExternalStore` compares with `Object.is` on every render, and a fresh object per
 * call is an infinite render loop. `sidebar/GitPanel/partialStore.ts` has the same note.
 */
import { diag, events, settings as settingsApi } from '@/ipc/client'
import type { DiffView, EditorSettings } from '@/ipc/client'

/**
 * The `EditorSettings` this window last saw, or `null` before the first answer arrives.
 *
 * The whole group is held, not just `diffView`, because `SettingsPatch` is per *top-level*
 * field — a caller changing one editor option sends the whole `EditorSettings` it currently
 * holds. Keeping only the one field would mean a toggle that resets `fontSize` and
 * `showMinimap` to whatever this file guessed.
 */
let editor: EditorSettings | null = null

/** The default before anything has been read. Matches `EditorSettings::default()` in Rust. */
const FALLBACK: DiffView = 'unified'

/**
 * What a subscriber sees.
 *
 * `writable` is in here rather than being a second function for a reason worth stating: the
 * stored mode is usually `unified`, which is also the fallback, so seeding changes `view` from
 * `unified` to `unified` — no change, no notification. A separate `diffViewWritable()` read
 * during render would therefore stay `false` for the life of the window in exactly the common
 * case, and the layout toggle would be permanently disabled. Both facts travel together, and a
 * new object is published whenever either moves.
 */
export interface DiffViewState {
  readonly view: DiffView
  /** False only until the first `settings.get` lands; see {@link setDiffView}. */
  readonly writable: boolean
}

const INITIAL: DiffViewState = { view: FALLBACK, writable: false }

let snapshot: DiffViewState = INITIAL
const listeners = new Set<() => void>()
/** Set once the seeding `settings.get()` has been issued; it is issued at most once. */
let seeded = false

function publish(next: EditorSettings): void {
  editor = next
  if (snapshot.view === next.diffView && snapshot.writable) return
  snapshot = { view: next.diffView, writable: true }
  for (const fn of listeners) fn()
}

/** Reference-stable between changes — `useSyncExternalStore` compares with `Object.is`. */
export function getDiffView(): DiffViewState {
  return snapshot
}

/** SSR sees the default. `renderToStaticMarkup` runs no effects, so nothing ever seeds it. */
export function getServerDiffView(): DiffViewState {
  return INITIAL
}

export function subscribeDiffView(onChange: () => void): () => void {
  listeners.add(onChange)

  if (!seeded) {
    seeded = true
    /*
     * Two sources, and both are needed. `settings.get` answers now — without it the first
     * diff of a launch paints unified for a round trip and then jumps. The event is what keeps
     * a second window, and the Settings screen, in step afterwards.
     *
     * Neither is awaited and neither throwing is fatal to the *pane*: the mode falls back to
     * unified, which is the default anyway, and every diff still renders.
     *
     * It is fatal to the toggle, deliberately. If `get` fails, `editor` stays `null`, so
     * `writable` stays false and both panes draw the control disabled. That is the honest
     * state: `setDiffView` cannot send a patch without the `EditorSettings` it is amending, and
     * sending one built from guesses would reset `fontSize`, `tabSize` and the rest to this
     * file's invented values — a settings-destroying write in exchange for a cosmetic toggle.
     */
    void settingsApi
      .get()
      .then((s) => publish(s.editor))
      .catch((e: unknown) => diag.log(`diff view: could not read settings: ${String(e)}`))

    void events
      .onWorkspaceChanged((ws) => publish(ws.settings.editor))
      .catch((e: unknown) => diag.log(`diff view: not following settings: ${String(e)}`))
  }

  return () => {
    listeners.delete(onChange)
  }
}

/**
 * Write the mode back.
 *
 * Published locally before the round trip, so the toggle moves on the click rather than an
 * IPC hop later — a segmented control that lags is one users press twice. The snapshot that
 * comes back on `cide://workspace-changed` is authoritative and overwrites this; if the write
 * failed, that snapshot is the old value and the control springs back, which is the honest
 * outcome and the only one that does not leave the pane disagreeing with `workspace.json`.
 *
 * A no-op before the settings have been read once. Sending a patch built on a guessed
 * `EditorSettings` would reset `fontSize` and friends to this file's guesses — see `editor`.
 */
export function setDiffView(next: DiffView): void {
  const current = editor
  if (current === null || current.diffView === next) return
  publish({ ...current, diffView: next })
  void settingsApi
    .set({ editor: { ...current, diffView: next } })
    .then((s) => publish(s.editor))
    .catch((e: unknown) => diag.log(`diff view: could not save the mode: ${String(e)}`))
}
