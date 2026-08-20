/**
 * The drag handle on the right edge of the left panel — file tree, git changes, search,
 * problems — and everything that keeps its width alive across a restart.
 *
 * **The gesture writes CSS, not React state.** `pointermove` sets one custom property on
 * `<html>` and commits to Rust once on `pointerup`, exactly as `layout/Splitter.tsx` does for
 * the pane grid and for the same reason: routing pointer moves through the store would
 * re-render `App` — the tab strip, the pane tree, every `PaneSlot` — on every move, and each
 * `PaneSlot` resize calls `fit()`, which reflows xterm and sends a SIGWINCH to a real child
 * process. One gesture would resize every live terminal dozens of times instead of once.
 * Nothing in this file calls a `set` between `pointerdown` and `pointerup` except the
 * `active` flag, which is local to this 6px element.
 *
 * Driving the **tokens** rather than the panels is what makes that possible without touching
 * a panel: `FileTree.module.css`, `GitPanel.module.css`, `SearchPanel.module.css` and
 * `ProblemsPanel.module.css` already say `width: var(--w-sidebar-…)`, so re-declaring the
 * property on `<html>` resizes whichever of them is mounted, with no React involved and no
 * second source of truth for the width. M18's Agents and Tasks panels join on the same terms:
 * one more token, `--w-sidebar-agents`, and nothing else here changes — which is the whole
 * argument for the token indirection.
 *
 * The decisions — the clamp, the defaults, the cache encoding — are in `./sidebarWidth.ts`,
 * which is import-free so `ui/scripts/check-sidebar.mjs` can compile and assert on it. What
 * is left here is the part that only exists in a browser.
 */
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { settings as settingsApi } from '@/ipc/client'
import { lockBodyForDrag, unlockBodyAfterDrag } from './dragLock'
import { useWorkspace } from '@/store/workspace'
import {
  SIDEBAR_CACHE_KEY,
  SIDEBAR_MIN,
  SIDEBAR_TOKEN,
  clampSidebarWidth,
  decodeWidths,
  encodeWidths,
  sidebarCeiling,
  toStored,
  widthDeclarations,
  widthFromDrag,
  widthsFromSettings,
  withPanel,
  type SidebarPanel,
  type SidebarWidths,
} from './sidebarWidth'
import styles from './SidebarSplitter.module.css'

/**
 * How far an arrow key moves the edge. 16px rather than 1px: a keyboard user resizing a
 * 252px panel to 420px should not need 168 keystrokes, and `Home`/`End` cover the ends.
 */
const KEY_STEP = 16

/**
 * The handle's accessible name, per panel.
 *
 * One name per *width*, not per view, which is why `files` is "the sidebar" rather than "the
 * file tree": that handle sizes the explorer, search and problems alike, and `agents` sizes
 * Agents and Tasks alike, so a name that promised one view would be wrong in the others.
 */
const PANEL_LABEL: Readonly<Record<SidebarPanel, string>> = {
  files: 'Resize the sidebar',
  git: 'Resize the git panel',
  agents: 'Resize the agents panel',
}

/** Matches `layout/Splitter.tsx`: long enough that key repeat coalesces, short enough to feel instant. */
const KEY_COMMIT_DELAY = 120

// --- window-scoped state ------------------------------------------------------------------
//
// Module scope, not component state, and deliberately: these widths belong to the *window*,
// not to whichever splitter happens to be mounted. Only one sidebar view is open at a time,
// so no two of these splitters are ever mounted together — but they hand the widths to each
// other across an unmount every time the user switches views, and a `useState` would reset
// them.

/** What this window has stored for every panel. Not viewport-clamped; see [`painted`]. */
let held: SidebarWidths = readCache()

/**
 * True between `pointerdown` and `pointerup`, at module scope rather than in a ref.
 *
 * The `resize` listener below is window-lifetime and has no component to ask, and repainting
 * from `held` mid-gesture would snap the edge back to where the drag started. Rare — a window
 * manager resizing the window while a button is held — but the failure is a visibly broken
 * drag, and one boolean is cheaper than reasoning about how rare it is.
 */
let gesturing = false

function readCache(): SidebarWidths {
  try {
    return decodeWidths(window.localStorage.getItem(SIDEBAR_CACHE_KEY))
  } catch {
    // Storage can be unavailable outright (a webview with it disabled, a private context).
    // The defaults are a correct first frame, so this is a missing optimisation, not an error.
    return widthsFromSettings(null)
  }
}

function writeCache(widths: SidebarWidths): void {
  try {
    const line = encodeWidths(widths)
    // Read-before-write, because the caller is the adopt effect and that runs on *every*
    // `cide://workspace-changed` — opening a tab, moving a pane, flipping any setting. The
    // widths are unchanged in all of those, and `setItem` in a WebKitGTK webview is a
    // synchronous hop to a SQLite-backed store. Nothing here is per-keystroke, but a disk
    // write per workspace mutation is a cost this feature has no reason to add.
    if (window.localStorage.getItem(SIDEBAR_CACHE_KEY) === line) return
    window.localStorage.setItem(SIDEBAR_CACHE_KEY, line)
  } catch {
    // Same: the width is already stored in `workspace.json`. Losing the cache costs one
    // frame of default width on the next launch, and nothing else.
  }
}

/** The width to actually draw: what is held, brought inside what this window can honour. */
function painted(widths: SidebarWidths): SidebarWidths {
  const available = window.innerWidth
  return {
    files: clampSidebarWidth(widths.files, available),
    git: clampSidebarWidth(widths.git, available),
    agents: clampSidebarWidth(widths.agents, available),
  }
}

/** Write every token onto `<html>`. The whole of how a width reaches the screen. */
function paint(widths: SidebarWidths): void {
  const root = document.documentElement
  for (const [property, value] of widthDeclarations(painted(widths))) {
    root.style.setProperty(property, value)
  }
}

/**
 * Restore before the first frame that has a sidebar in it.
 *
 * A module side effect, which is unusual enough to say why. This module is imported by
 * `App.tsx`, which is imported by `main.tsx`; ES module evaluation completes before
 * `createRoot().render()` runs, so the tokens are already re-declared when React lays the
 * panel out for the first time. An effect — even a layout effect — runs after that first
 * layout, and the panel would be seen at 252px and then jump.
 *
 * `theme-boot.js` is the precedent and was read before this was written; `decodeWidths`
 * explains why this cannot use its `?theme=`-style channel and what the difference is.
 */
paint(held)

/**
 * Re-fit on window resize.
 *
 * The stored width is deliberately *not* changed here. A user who narrows the window has not
 * asked for a narrower file tree forever — they have asked for a narrower window — so the
 * ceiling applies to what is drawn and the held value survives to be restored when the
 * window is wide again. Persisting the squeeze would silently destroy the setting.
 *
 * At module scope rather than in an effect for the reason `main.tsx` installs
 * `installThemeSync` there: this is window-lifetime, StrictMode's double-mount cannot install
 * it twice, and no unmount can tear it down while the window is still open.
 */
window.addEventListener('resize', () => {
  if (!gesturing) paint(held)
})

// --- the component --------------------------------------------------------------------

export interface SidebarSplitterProps {
  /** Which panel this handle sizes. Selects the token, the stored field and the label. */
  panel: SidebarPanel
}

export function SidebarSplitter({ panel }: SidebarSplitterProps) {
  const stored = useWorkspace((s) => s.boot?.workspace.settings.sidebar ?? null)

  const [active, setActive] = useState(false)
  const dragging = useRef(false)
  /** Where the gesture began: the pointer, and the width under it. */
  const origin = useRef({ x: 0, width: 0 })
  /** The width the DOM is currently showing. Diverges from `held` for the length of a drag. */
  const live = useRef(held[panel])
  const selfRef = useRef<HTMLDivElement>(null)
  const commitTimer = useRef<number | null>(null)

  /**
   * Adopt the workspace's widths.
   *
   * This is what makes a resize in one window reach another: settings ride every
   * `cide://workspace-changed` snapshot, so a commit anywhere lands here. Skipped while this
   * splitter is dragging, or the snapshot answering the *previous* gesture would yank the
   * edge out from under the current one.
   *
   * A *layout* effect, so the mount case has no visible frame either. The module-level paint
   * covers the first render of the window; this covers the render where a panel is opened
   * for the first time in a session whose boot cache was stale — after the DOM is built,
   * before the browser paints it.
   */
  useLayoutEffect(() => {
    if (dragging.current || !stored) return
    held = widthsFromSettings(stored)
    // The *painted* width, not the held one, for the same reason `pointerdown` takes it: on a
    // window too narrow to honour the stored value the two differ, and the first arrow key
    // would then step from a number that is not on screen — moving nothing, and committing
    // the clamped value over the stored one for no gesture the user made.
    live.current = painted(held)[panel]
    writeCache(held)
    paint(held)
  }, [stored, panel])

  useEffect(
    () => () => {
      if (commitTimer.current !== null) window.clearTimeout(commitTimer.current)
    },
    [],
  )

  // A view switch can unmount this mid-drag. Without this the body keeps the selection lock
  // and a resize cursor for the rest of the session.
  useEffect(
    () => () => {
      if (dragging.current) {
        gesturing = false
        unlockBodyAfterDrag()
      }
    },
    [],
  )

  /**
   * Send the width to Rust, and to the boot cache in the same breath.
   *
   * Fire and forget, like every other settings write: the answer arrives as a snapshot, and
   * awaiting it would make the next drag wait on an IPC round trip. A rejected write leaves
   * this window wider than `workspace.json` knows about until the next snapshot corrects it,
   * which is the same trade every other setting makes.
   */
  const commit = useCallback(
    (width: number) => {
      held = withPanel(held, panel, width, window.innerWidth)
      writeCache(held)
      void settingsApi.set({ sidebar: toStored(held) }).catch(() => {})
    },
    [panel],
  )

  const stop = useCallback(
    (doCommit: boolean) => {
      if (!dragging.current) return
      dragging.current = false
      gesturing = false
      setActive(false)
      unlockBodyAfterDrag()
      if (doCommit) commit(live.current)
    },
    [commit],
  )

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return
    // Or the gesture starts a text selection in whatever it began over.
    e.preventDefault()
    e.currentTarget.setPointerCapture(e.pointerId)
    // The *painted* width, not the held one: on a window too narrow to honour the stored
    // value the two differ, and starting from the held one would make the first move jump.
    origin.current = { x: e.clientX, width: painted(held)[panel] }
    live.current = origin.current.width
    dragging.current = true
    gesturing = true
    setActive(true)
    // On the body, not on this element: the pointer spends the drag over the panel and the
    // panes, and without this the cursor flickers to a text caret on every crossing. Through
    // `dragLock` because the selection half of it cannot be written as `style.userSelect` in
    // this engine and silently appears to work — see that module.
    lockBodyForDrag('col-resize')
  }

  /** Move the edge. No React, no store, one custom property. */
  const show = (width: number) => {
    live.current = width
    document.documentElement.style.setProperty(SIDEBAR_TOKEN[panel], `${width}px`)
    selfRef.current?.setAttribute('aria-valuenow', String(width))
  }

  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!dragging.current) return
    show(widthFromDrag(origin.current.width, e.clientX - origin.current.x, window.innerWidth))
  }

  /**
   * Move now, tell the domain shortly — the keyboard half of the same bargain.
   *
   * A held arrow key repeats at roughly 30 Hz, and a commit per repeat is the re-render storm
   * the pointer path exists to avoid, arriving through the keyboard instead.
   */
  const moveTo = (width: number) => {
    show(clampSidebarWidth(width, window.innerWidth))
    if (commitTimer.current !== null) window.clearTimeout(commitTimer.current)
    commitTimer.current = window.setTimeout(() => {
      commitTimer.current = null
      commit(live.current)
    }, KEY_COMMIT_DELAY)
  }

  /** Send a pending keyboard commit immediately, so none is ever dropped to a blur. */
  const flushCommit = () => {
    if (commitTimer.current === null) return
    window.clearTimeout(commitTimer.current)
    commitTimer.current = null
    commit(live.current)
  }

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (e.key === 'ArrowLeft') moveTo(live.current - KEY_STEP)
    else if (e.key === 'ArrowRight') moveTo(live.current + KEY_STEP)
    else if (e.key === 'Home') moveTo(0)
    else if (e.key === 'End') moveTo(sidebarCeiling(window.innerWidth))
    else return
    e.preventDefault()
  }

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label={PANEL_LABEL[panel]}
      aria-valuenow={painted(held)[panel]}
      aria-valuemin={SIDEBAR_MIN}
      aria-valuemax={sidebarCeiling(window.innerWidth)}
      tabIndex={0}
      ref={selfRef}
      data-audit="sidebar-splitter"
      data-panel={panel}
      className={`${styles.splitter} ${active ? styles.splitterActive : ''}`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={() => stop(true)}
      onPointerCancel={() => stop(false)}
      // Safety net: also fires after `pointerup`, where `stop` has already run and returns.
      onLostPointerCapture={() => stop(false)}
      onKeyDown={onKeyDown}
      onKeyUp={flushCommit}
      onBlur={flushCommit}
    />
  )
}
