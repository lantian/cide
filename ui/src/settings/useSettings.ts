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
 * is installed once per window from `main.tsx`, before the first render.
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
  session as sessionApi,
  settings as settingsApi,
  type Settings,
  type SettingsPatch,
  type WindowMode,
} from '@/ipc/client'
import { useWorkspace, type Theme } from '@/store/workspace'
import { liveHosts } from '@/layout/paneHosts'
import { fontVariables } from './fontScale'
import { refont, retheme } from '@/terminal/xterm'
import { asThemeName, DEFAULT_THEME, otherTheme, themeToAdopt } from './theme'

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
 * snapshot. `themeToAdopt` is what keeps the subscription from undoing the local half
 * before the snapshot gets back.
 *
 * The failure is not swallowed, unlike [`useSettingsActions.patch`]. Every other setting is
 * shown from the mirror, so a write that never landed simply fails to appear; the theme is
 * the one setting painted locally first, and a rejected write would otherwise leave this
 * window wearing a palette no other window has and `workspace.json` does not know about —
 * the reported bug, arrived at from the other end. Put the mirror's answer back instead.
 */
export function applyTheme(theme: Theme): void {
  useWorkspace.getState().setTheme(theme)
  void settingsApi.set({ theme }).catch(() => {
    const stored = useWorkspace.getState().boot?.workspace.settings.theme
    if (stored !== undefined) useWorkspace.getState().setTheme(stored)
  })
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
 * Write the font tokens the two Settings controls decide, and repaint what is already open.
 *
 * The mirror of [`paintTheme`], and it exists for the same reason: a token change reaches the
 * editor through the cascade and reaches a live terminal through nothing. What made these two
 * controls appear dead was not an unwired `onChange` — the values were stored correctly — but
 * that nothing turned them into `--fs-code`/`--lh-code`/`--fs-term`/`--term-line-height`, two
 * of which cannot be derived in CSS at all. See `fontScale.ts`.
 *
 * Guarded on the resolved values rather than on the settings object, so the store changes that
 * are not font changes — which is nearly all of them — cost four string compares and no work.
 */
function paintFonts(editorSize: number, terminalSize: number): void {
  const root = document.documentElement
  const vars = fontVariables(editorSize, terminalSize, window.devicePixelRatio || 1)
  let moved = false
  for (const [name, value] of Object.entries(vars)) {
    if (root.style.getPropertyValue(name) === value) continue
    root.style.setProperty(name, value)
    moved = true
  }
  if (!moved) return
  // After the properties, never before, and for `retheme`'s reason: `refont` reads the tokens
  // back off `<html>` with `getComputedStyle`, so it has to observe the ones just written.
  refont(
    [...liveHosts()].flatMap((h) => (h.terminal ? [h.terminal] : [])),
    (handle) => {
      // The child has to be told, or it keeps writing at the old width and every wrapped line
      // is wrong. Found by pane, because `refont` deliberately knows nothing about sessions.
      for (const host of liveHosts()) {
        if (host.terminal === handle && host.sessionId) {
          const cell = handle.cellSize()
          host.lastGeometry = undefined
          void sessionApi
            .resize(host.sessionId, {
              cols: handle.term.cols,
              rows: handle.term.rows,
              cellWidth: cell.width,
              cellHeight: cell.height,
            })
            .catch(() => {})
        }
      }
    },
  )
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
 * Called from `main.tsx`, before `createRoot`, and not from `App`: an effect would run after
 * the first render, which is after `TerminalPane` can have built a terminal, and this has to
 * be the thing that settles what palette that terminal reads. `main.tsx` also makes it
 * unconditional — a component-level call is one refactor away from being dropped, which is
 * how this function came to be exported, documented and called by nothing.
 *
 * Returns an unsubscribe function. Idempotent: a second caller gets a no-op rather than the
 * installer's disposer, because a component that installed nothing must not be able to tear
 * down the window's only theme listener when it unmounts.
 */
export function installThemeSync(): () => void {
  if (themeSyncInstalled !== null) return noop

  // Before the subscription, because the first paint of the window is not a store change.
  //
  // The attribute wins over the store's seed when it is there. `public/theme-boot.js` writes
  // it in <head> from the `?theme=` parameter `windows.rs` bakes in from the saved settings,
  // so it is the persisted theme, known a whole module graph earlier than `hydrate` can say
  // it. Painting the seed over it would flash the wrong palette on every launch — and would
  // hand any terminal built before `hydrate` the wrong resolved colours, which `retheme`
  // then has to undo.
  const start = asThemeName(document.documentElement.dataset.theme) ?? DEFAULT_THEME
  useWorkspace.getState().setTheme(start)
  paintTheme(start)
  const bootSettings = useWorkspace.getState().boot?.workspace.settings
  if (bootSettings) paintFonts(bootSettings.editor.fontSize, bootSettings.terminal.fontSize)

  const unsubscribe = useWorkspace.subscribe((state, previous) => {
    const adopt = themeToAdopt(
      state.theme,
      state.boot?.workspace.settings.theme,
      previous.boot?.workspace.settings.theme,
    )
    if (adopt !== null) {
      // Re-enters this listener synchronously with the field moved, and `themeToAdopt`
      // answers `null` there, so the paint happens on the way through and once.
      state.setTheme(adopt)
      return
    }
    paintTheme(state.theme)
    const settings = state.boot?.workspace.settings
    if (settings) paintFonts(settings.editor.fontSize, settings.terminal.fontSize)
  })

  themeSyncInstalled = () => {
    themeSyncInstalled = null
    unsubscribe()
  }
  return themeSyncInstalled
}

const noop = (): void => {}

let themeSyncInstalled: (() => void) | null = null
