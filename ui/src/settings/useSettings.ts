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
  const setLocalTheme = useWorkspace((s) => s.setTheme)
  const setMode = useWorkspace((s) => s.setWindowMode)

  const patch = useCallback((patch: SettingsPatch) => {
    // Fire and forget. Every outcome the caller could branch on already arrives as a
    // snapshot, and awaiting here would make a toggle wait an IPC round trip before it moved.
    void settingsApi.set(patch).catch(() => {})
  }, [])

  const setTheme = useCallback(
    (theme: Theme) => {
      setLocalTheme(theme)
      void settingsApi.set({ theme }).catch(() => {})
    },
    [setLocalTheme],
  )

  const setWindowMode = useCallback(
    (mode: WindowMode) => {
      void setMode(mode)
    },
    [setMode],
  )

  return { patch, setTheme, setWindowMode }
}
