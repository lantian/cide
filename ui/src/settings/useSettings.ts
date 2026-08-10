/**
 * The Settings screen's bridge to the Rust-owned workspace.
 *
 * Settings are a field of the workspace, so they arrive with every snapshot and there is
 * nothing local to keep in step: a write goes to Rust, comes back on
 * `cide://workspace-changed`, and the screen re-renders from the mirror like every other
 * surface. No optimistic local copy — two windows can have this screen open, and a local copy
 * is how they would start disagreeing.
 *
 * The one exception is the theme, and it is a real one. `document.documentElement.dataset.theme`
 * is driven by a plain field in the workspace store, and xterm terminals hold a *resolved*
 * colour table rather than CSS variables, so they need repainting explicitly. Both are done
 * eagerly here, before the round trip, because a theme that flips a frame late reads as a
 * broken switch. [`installThemeSync`] is the single place that does the repainting, for the
 * window that made the change and for the windows that only hear about it in a snapshot; it
 * must be installed once per window, from `App`.
 *
 * That exception is also why [`applyTheme`] and [`installThemeSync`] are exported as plain
 * functions rather than living inside `useSettingsActions`. The theme is changed from two
 * places — this screen and the header's toggle — and the header is not a settings component,
 * so a hook could not be its route. Before this, it had its own route: `useWorkspace.toggleTheme`
 * set the local field and stopped there, which is why the header's switch disagreed with
 * Settings → Appearance, never reached a second window, and did not survive a relaunch.
 */
import { useCallback } from 'react'
import {
  settings as settingsApi,
  type Settings,
  type SettingsPatch,
  type WindowMode,
} from '@/ipc/client'
import { useWorkspace, type Theme } from '@/store/workspace'
import { liveHosts } from '@/layout/paneHosts'
import { retheme } from '@/terminal/xterm'
import { otherTheme, themeToAdopt } from './theme'

/** The settings this window currently mirrors, or `null` before bootstrap resolves. */
export function useSettings(): Settings | null {
  return useWorkspace((s) => s.boot?.workspace.settings ?? null)
}

export interface SettingsActions {
  /** Apply a partial update. Fields left out are untouched. */
  patch: (patch: SettingsPatch) => void
  /** Theme, applied to the DOM and to live terminals before the round trip. */
  setTheme: (theme: Theme) => void
  /** Stacked or one-window-per-project. Not a patch: it opens and closes OS windows. */
  setWindowMode: (mode: WindowMode) => void
}

export function useSettingsActions(): SettingsActions {
  const setMode = useWorkspace((s) => s.setWindowMode)

  const patch = useCallback((patch: SettingsPatch) => {
    // Fire and forget. Every outcome the caller could branch on already arrives as a
    // snapshot, and awaiting here would make a toggle wait an IPC round trip before it moved.
    void settingsApi.set(patch).catch(() => {})
  }, [])

  const setWindowMode = useCallback(
    (mode: WindowMode) => {
      void setMode(mode)
    },
    [setMode],
  )

  return { patch, setTheme: applyTheme, setWindowMode }
}

/**
 * Set the theme everywhere it is held: the local store now, `workspace.json` in a moment.
 *
 * A plain function rather than a hook, because the header's toggle is not on this screen and
 * cannot call a hook that lives in it. That was the whole bug: `AppHeader`'s button went to
 * `useWorkspace.toggleTheme`, which only ever did `set({ theme })`. The result was a switch
 * that worked until you looked at it twice — Settings → Appearance still showed the old
 * value, a second window never heard about the change at all, and the next launch restored
 * whatever `workspace.json` still said. There is now exactly one way to change the theme and
 * this is it.
 *
 * Local first, then the round trip: the DOM attribute and the terminals' resolved colour
 * tables are repainted by [`installThemeSync`]'s subscription, synchronously inside this
 * `set`, and a theme that flips a frame late reads as a broken switch. The round trip is
 * what carries it to the *other* windows, where the same subscription picks it out of the
 * snapshot.
 */
export function applyTheme(theme: Theme): void {
  useWorkspace.getState().setTheme(theme)
  void settingsApi.set({ theme }).catch(() => {})
}

/** Flip to the other theme and persist it. What the header's button is for. */
export function toggleTheme(): void {
  applyTheme(otherTheme(useWorkspace.getState().theme))
}

/**
 * Paint the theme onto everything in this window that does not follow a CSS variable.
 *
 * Two surfaces, and only two, once the tokens are in place:
 *
 * - `<html data-theme>`, which is what selects the palette for all the CSS. Writing it also
 *   ticks the `MutationObserver` in `editor/minimap.ts`, which repaints the canvas — a
 *   canvas cannot read a custom property, so that is the minimap's whole theme story.
 *   `editor/highlight.ts` needs nothing: it emits classes and `EditorSurface.module.css`
 *   resolves them through `var(--purple)` and friends at paint time, so the buffer follows
 *   for free. That was checked before writing this, not assumed — a `HighlightStyle` built
 *   with literal colours would have been baked in at construction and would need the
 *   extension reconfigured instead.
 * - Live xterm terminals, which hold a *resolved* colour table. See [`retheme`].
 *
 * Guarded on the attribute rather than on a remembered value, so it is idempotent and
 * costs one string compare on the store changes that are not theme changes — which is
 * almost all of them, since the subscription below sees every snapshot.
 */
function paintTheme(theme: Theme): void {
  const root = document.documentElement
  if (root.dataset.theme === theme) return
  root.dataset.theme = theme
  // After the attribute, never before: `retheme` reads the tokens back off `<html>` with
  // `getComputedStyle`, which resolves against the palette that is selected *now*.
  retheme([...liveHosts()].flatMap((h) => (h.terminal ? [h.terminal] : [])))
}

/**
 * Follow the theme in the workspace mirror, and repaint this window whenever it moves.
 *
 * The other half of the same bug, and the one the user is reporting: "switching theme isn't
 * working correctly - it doesn't switch all claude/terminal windows". `hydrate` seeds
 * `theme` from the bootstrap and nothing updated it afterwards — `applySnapshot` replaces
 * `boot`, settings included, but the field that drives `data-theme` is separate from it and
 * stayed put. A window that did not make the change kept its old theme until it was
 * restarted, and every terminal in it with it. This function existed for that and **nothing
 * called it**, so none of it ran.
 *
 * It also owns the painting, rather than leaving that to an effect in `App`. Two reasons.
 * The store is where a theme change actually happens, from either of the two routes into
 * `applyTheme` and now from a snapshot as well, so a subscription sees all three and runs
 * synchronously inside the `set` — no frame of the old palette on the way through. And it
 * puts the DOM write and the terminal repaint in the same file as the mirror-following
 * rule, which is what stops the next window-level surface being repainted in one path and
 * not the other. `App`'s own theme effect becomes redundant, and harmless if kept:
 * [`paintTheme`] is idempotent and `retheme` skips terminals already wearing the colours.
 *
 * A store subscription rather than an effect per component: the store is the thing that
 * changes, there is one of it per window, and this way exactly one place is listening.
 *
 * Returns an unsubscribe function. Idempotent — calling it twice installs one subscription —
 * so a `StrictMode` double-mount cannot end up with two.
 */
export function installThemeSync(): () => void {
  if (themeSyncInstalled !== null) return themeSyncInstalled

  // Before the subscription, because the first paint of the window is not a store change.
  // `index.html` carries no `data-theme`, so until this runs the document is on whatever
  // bare `:root` says, and any terminal built in the meantime has read those tokens.
  paintTheme(useWorkspace.getState().theme)

  const unsubscribe = useWorkspace.subscribe((state) => {
    const adopt = themeToAdopt(state.theme, state.boot?.workspace.settings.theme)
    if (adopt !== null) {
      // Re-enters this listener synchronously with the field moved, and `themeToAdopt`
      // answers `null` there, so the paint happens on the way through and once.
      state.setTheme(adopt)
      return
    }
    paintTheme(state.theme)
  })

  themeSyncInstalled = () => {
    themeSyncInstalled = null
    unsubscribe()
  }
  return themeSyncInstalled
}

let themeSyncInstalled: (() => void) | null = null
