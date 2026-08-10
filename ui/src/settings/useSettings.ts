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
 * broken switch.
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
 * tables are repainted by `App`'s effect on the store's `theme` field, and a theme that
 * flips a frame late reads as a broken switch.
 */
export function applyTheme(theme: Theme): void {
  useWorkspace.getState().setTheme(theme)
  void settingsApi.set({ theme }).catch(() => {})
}

/** Flip to the other theme and persist it. What the header's button is for. */
export function toggleTheme(): void {
  applyTheme(useWorkspace.getState().theme === 'dark' ? 'light' : 'dark')
}

/**
 * Follow the theme in the workspace mirror, so a change made in *another* window lands here.
 *
 * The second half of the same bug. `hydrate` seeds `theme` from the bootstrap and nothing
 * updates it afterwards: `applySnapshot` replaces `boot` — settings included — but the field
 * that drives `document.documentElement.dataset.theme` is separate from it and stayed put. A
 * window that did not make the change kept its old theme until it was restarted.
 *
 * A store subscription rather than an effect in each component: the store is the thing that
 * changes, there is one of it per window, and this way exactly one place is listening.
 *
 * Returns an unsubscribe function. Idempotent — calling it twice installs one subscription —
 * so a `StrictMode` double-mount cannot end up with two.
 */
export function installThemeSync(): () => void {
  if (themeSyncInstalled !== null) return themeSyncInstalled

  const unsubscribe = useWorkspace.subscribe((state) => {
    const stored = state.boot?.workspace.settings.theme
    // `stored === state.theme` is the steady state and covers the window that made the
    // change: `applyTheme` moved the local field before the snapshot arrived carrying the
    // same value, so there is nothing to do and no second render to cause.
    if (stored !== undefined && stored !== state.theme) state.setTheme(stored)
  })

  themeSyncInstalled = () => {
    themeSyncInstalled = null
    unsubscribe()
  }
  return themeSyncInstalled
}

let themeSyncInstalled: (() => void) | null = null
