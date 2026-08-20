/**
 * The drag handle on the **top** edge of the git tool window, and everything that keeps its
 * height alive across a restart.
 *
 * The vertical twin of `chrome/SidebarSplitter.tsx`, and it copies that gesture deliberately
 * rather than sharing it. What is shared is the *decision module* (`toolWindowHeight.ts`) and the
 * one genuinely common hazard (`chrome/dragLock.ts`); the gesture itself differs in the four
 * places that matter — the sign is inverted, there is one token instead of three, the ceiling is
 * measured against the window's *height* minus three chrome bars, and the aria is horizontal. A
 * hook covering both would take a callback for the write, one for the clamp, one for the commit,
 * an element ref and an orientation, at which point its body is `useState(false)` and it has
 * abstracted nothing while giving two surfaces one failure mode. `SidebarSplitter.tsx`'s own
 * header states that position; this is the second file to follow it.
 *
 * **The gesture writes CSS, not React state**, and here that is not merely an optimisation.
 * Every `pointermove` changes the pane area's height, `layout/PaneSlot.tsx` runs a
 * `ResizeObserver` that calls `fit()`, and `fit()` reflows xterm and sends a SIGWINCH to a live
 * `claude`. Routing moves through the store would additionally re-render `App` — the tab strip,
 * the pane tree, every `PaneSlot` — dozens of times in one drag. Nothing between `pointerdown`
 * and `pointerup` touches the store except the `active` flag, which is local to this 6px element.
 *
 * (That paragraph named a cost it then accepted, and the acceptance was wrong. The cell-size
 * comparison bounds the `session_resize`, not the `fit()` that precedes it — and `fit()` is a
 * DOM-renderer reflow over 5000 lines of scrollback, run for every pane of *every* tab, since
 * `layout/TabContent.module.css` lays the hidden ones out at full size too. `@/layout/resizeGesture`
 * is where that is now dealt with: the drag is bracketed as a gesture and the whole refit happens
 * once, on release. The decision that the terminal grid does not follow the drag is written down
 * in that module.)
 *
 * The write itself is coalesced onto one `requestAnimationFrame`, and goes through [`writeToken`]
 * so an unchanged value costs nothing. Both matter more here than the line count suggests: the
 * token is an inherited custom property on `<html>`, so every write invalidates style for the
 * whole document, and WebKitGTK reports `pointermove` at the mouse's rate rather than the frame
 * rate.
 */
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { lockBodyForDrag, unlockBodyAfterDrag } from '@/chrome/dragLock'
import { beginResizeGesture, endResizeGesture } from '@/layout/resizeGesture'
import { toolWindow as toolWindowApi, windowLabel } from '@/ipc/client'
import { useWorkspace } from '@/store/workspace'
import type { ProjectId } from '@/ipc/client'
import {
  TOOL_CACHE_KEY,
  TOOL_MIN,
  TOOL_TOKEN,
  clampToolWindowHeight,
  decodeToolWindow,
  encodeToolWindow,
  geometryFromState,
  heightDeclaration,
  heightFromDrag,
  toolWindowCeiling,
  type ToolWindowGeometry,
} from './toolWindowHeight'
import styles from './ToolWindowSplitter.module.css'
import { currentUiScale, useUiScale } from '@/settings/useUiScale'

/** How far an arrow key moves the edge. Matches `SidebarSplitter`'s 16px. */
const KEY_STEP = 16

/** Matches `SidebarSplitter` and `layout/Splitter.tsx`: long enough that key repeat coalesces. */
const KEY_COMMIT_DELAY = 120

// --- window-scoped state ------------------------------------------------------------------
//
// Module scope for the same reason the sidebar's widths are: this height belongs to the
// *window*. The panel unmounts whenever it is closed, and a `useState` would forget the height
// every time — so the very first thing a reopen did would be to jump.

/** What this window is showing. Not viewport-clamped; see [`painted`]. */
let held: ToolWindowGeometry = readCache()

/** True between `pointerdown` and `pointerup`. See `SidebarSplitter.tsx` for why not a ref. */
let gesturing = false

function readCache(): ToolWindowGeometry {
  try {
    return decodeToolWindow(window.localStorage.getItem(TOOL_CACHE_KEY), windowLabel())
  } catch {
    // Storage can be unavailable outright (a webview with it disabled, a private context). The
    // default is a correct first frame, so this is a missing optimisation, not an error.
    return geometryFromState(null)
  }
}

function writeCache(geometry: ToolWindowGeometry, liveLabels: readonly string[]): void {
  try {
    const raw = window.localStorage.getItem(TOOL_CACHE_KEY)
    const line = encodeToolWindow(raw, windowLabel(), geometry, liveLabels)
    // Read-before-write: the caller is the adopt effect, which runs on *every*
    // `cide://workspace-changed` — opening a tab, moving a pane, flipping any setting. In all of
    // those the geometry is unchanged, and `setItem` in a WebKitGTK webview is a synchronous hop
    // to a SQLite-backed store.
    if (raw === line) return
    window.localStorage.setItem(TOOL_CACHE_KEY, line)
  } catch {
    // The height is already in `workspace.json`. Losing the cache costs one frame on the next
    // launch and nothing else.
  }
}

/**
 * The height to actually draw: what is held, brought inside what this window can honour.
 *
 * `currentUiScale()` rather than a hook because this runs from module scope — `paint` below is
 * called before React's first commit so the token is on `<html>` for the first frame. The
 * chrome rows this ceiling subtracts scale with the chrome font size, so a flat ceiling here
 * would let the panel take room the panes are entitled to at any size but the default.
 */
function painted(geometry: ToolWindowGeometry): number {
  return clampToolWindowHeight(geometry.height, window.innerHeight, currentUiScale())
}

/**
 * What each token was last set to, so an unchanged value costs nothing.
 *
 * Keyed by property even though there is exactly one today: the comparison is only sound if it is
 * per property, and a second token added later would otherwise silently suppress the first.
 *
 * It is an inherited custom property on the root element: writing one invalidates style for the
 * whole document, and most of this document is terminal rows. `paint` runs on every workspace
 * snapshot and on every frame of a window resize, and in nearly all of those the height has not
 * moved.
 */
const written = new Map<string, string>()

function writeToken(property: string, value: string): void {
  if (written.get(property) === value) return
  written.set(property, value)
  document.documentElement.style.setProperty(property, value)
}

/** Write the token onto `<html>`. The whole of how the height reaches the screen. */
function paint(geometry: ToolWindowGeometry): void {
  const [property, value] = heightDeclaration(painted(geometry))
  writeToken(property, value)
}

/**
 * Restore before the first frame that has the panel in it.
 *
 * A module side effect, for the reason `SidebarSplitter.tsx` gives at length: this module is
 * imported by `App.tsx`, which is imported by `main.tsx`, and ES module evaluation completes
 * before `createRoot().render()`. An effect — even a layout effect — runs after the first layout,
 * and the panel would be laid out at the default and then jump, taking every terminal's `fit()`
 * with it.
 */
paint(held)

/**
 * Re-fit on window resize. The *stored* height is deliberately not changed: a user who shortens
 * the window has not asked for a shorter tool window for ever, so the ceiling applies to what is
 * drawn and the held value survives to be restored when the window is tall again.
 */
window.addEventListener('resize', () => {
  if (!gesturing) paint(held)
})

/** What the rest of the app reads to decide whether to render the panel at all. */
export function cachedToolWindowOpen(): boolean {
  return held.open
}

// --- the component --------------------------------------------------------------------------

export interface ToolWindowSplitterProps {
  /** Whose panel this is. The height is per project — see `cide_core::toolwindow`. */
  project: ProjectId
}

export function ToolWindowSplitter({ project }: ToolWindowSplitterProps) {
  const stored = useWorkspace((s) => {
    const p = s.boot?.workspace.projects[project]
    return p?.toolWindow ?? null
  })
  /*
   * The live window labels, and the `useMemo` is not tidiness — it is the whole bug.
   *
   * A zustand selector runs on **every** store read, and `useSyncExternalStore` compares what it
   * returns with `Object.is`. `useWorkspace((s) => Object.keys(...))` therefore hands React a
   * fresh array every single time, React concludes the snapshot changed, re-renders, reads
   * again, gets another fresh array — and the loop only ends when React gives up with *Maximum
   * update depth exceeded*. Because this splitter is rendered exactly when the tool window is
   * open, and `open` is persisted on `Project`, that took the whole window down on every launch.
   *
   * So the selector returns the **stored record**, whose identity is stable until the workspace
   * really changes, and the derivation happens outside it. `FileTree.tsx` reaches for
   * `useShallow` for the same hazard; that works too, and this is the cheaper answer where the
   * source is already one stable object and only the projection is new.
   */
  const windows = useWorkspace((s) => s.boot?.workspace.windows)
  const labels = useMemo(() => Object.keys(windows ?? {}), [windows])

  const [active, setActive] = useState(false)
  /*
   * The chrome above and below this panel is `calc(… * var(--ui-scale))`, so the room left for
   * the panes is too. Without this the ceiling would be computed against the chrome's *design*
   * height at every setting, and at 17px it would let the panel take 22px it does not have —
   * out of `MIN_PANES`, on tall windows only. See `toolWindowCeiling`.
   */
  const scale = useUiScale()
  const dragging = useRef(false)
  /** Where the gesture began: the pointer, and the height under it. */
  const origin = useRef({ y: 0, height: 0 })
  /** The height the DOM is showing. Diverges from `held` for the length of a drag. */
  const live = useRef(held.height)
  const selfRef = useRef<HTMLDivElement>(null)
  const commitTimer = useRef<number | null>(null)
  /** The pending token write, so a burst of moves paints once. */
  const frame = useRef<number | null>(null)
  /** True while a run of arrow-key nudges is open. See `SidebarSplitter`'s twin. */
  const keying = useRef(false)

  /**
   * Adopt the workspace's geometry.
   *
   * A *layout* effect, so the mount case has no visible frame either: the module-level paint
   * covers the window's first render, this covers the render where the panel is opened for the
   * first time in a session whose boot cache was stale. Skipped while dragging, or the snapshot
   * answering the *previous* gesture would yank the edge out from under the current one.
   */
  useLayoutEffect(() => {
    if (dragging.current || !stored) return
    held = geometryFromState(stored)
    // The *painted* height, not the held one: on a window too short to honour the stored value
    // the two differ, and the first arrow key would step from a number that is not on screen.
    live.current = painted(held)
    writeCache(held, labels)
    paint(held)
  }, [stored, labels])

  useEffect(
    () => () => {
      if (commitTimer.current !== null) window.clearTimeout(commitTimer.current)
    },
    [],
  )

  // Closing the panel can unmount this mid-drag. Without this the body keeps the selection lock
  // and a resize cursor for the rest of the session.
  useEffect(
    () => () => {
      if (frame.current !== null) cancelAnimationFrame(frame.current)
      if (dragging.current) {
        dragging.current = false
        gesturing = false
        unlockBodyAfterDrag()
        // Left open, this would stop every terminal in the window refitting until
        // `resizeGesture`'s watchdog noticed. Closing the panel can land here mid-drag.
        endResizeGesture()
      }
      if (keying.current) {
        keying.current = false
        endResizeGesture()
      }
    },
    [],
  )

  /**
   * Send the height to Rust, and to the boot cache in the same breath.
   *
   * Fire and forget: the answer arrives as a snapshot, and awaiting it would make the next drag
   * wait on a round trip. Rust no-ops a patch that changes nothing, so committing
   * unconditionally on `pointerup` costs no broadcast for a drag that ended where it started.
   */
  const commit = useCallback(
    (height: number) => {
      held = { ...held, height: clampToolWindowHeight(height, window.innerHeight, scale) }
      writeCache(held, labels)
      void toolWindowApi.setLayout(project, { height: held.height }).catch(() => {})
    },
    // `scale` is in the deps because `commit` clamps with it: held across a settings change,
    // this callback would clamp the committed height against the ceiling of a chrome size the
    // window is no longer wearing.
    [project, labels, scale],
  )

  const stop = (doCommit: boolean) => {
    if (!dragging.current) return
    dragging.current = false
    gesturing = false
    setActive(false)
    // The last pointer event may not have been painted yet, and the gesture is over: put the
    // height being committed on screen rather than letting a frame land after the model moved.
    settle()
    unlockBodyAfterDrag()
    endResizeGesture()
    if (doCommit) commit(live.current)
  }

  const onPointerDown = (e: React.PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return
    // Or the gesture starts a text selection in whatever it began over.
    e.preventDefault()
    e.currentTarget.setPointerCapture(e.pointerId)
    // The *painted* height, not the held one: on a short window the two differ and starting from
    // the held one would make the first move jump.
    origin.current = { y: e.clientY, height: painted(held) }
    live.current = origin.current.height
    dragging.current = true
    gesturing = true
    setActive(true)
    // Before the first move, so nothing downstream refits on the frames this drag is about to
    // produce. Paired in `stop` and in the unmount effect.
    beginResizeGesture()
    // On the body, not on this element: the pointer spends the drag over the panes and the panel.
    // Through `dragLock` because the selection half cannot be written as `style.userSelect` in
    // this engine and silently appears to work — see that module.
    lockBodyForDrag('row-resize')
  }

  /** Put `live.current` on screen. One custom property, plus the aria the gesture owes. */
  const paintLive = () => {
    writeToken(TOOL_TOKEN, `${live.current}px`)
    selfRef.current?.setAttribute('aria-valuenow', String(live.current))
  }

  /**
   * Move the edge. No React, no store, one custom property — on the next frame.
   *
   * The height is recorded synchronously and only the write waits, so `stop` can commit
   * `live.current` without a frame having had to land first.
   */
  const show = (height: number) => {
    live.current = height
    if (frame.current !== null) return
    frame.current = requestAnimationFrame(() => {
      frame.current = null
      paintLive()
    })
  }

  /** Drop a scheduled write and put the final position on screen now. */
  const settle = () => {
    if (frame.current !== null) {
      cancelAnimationFrame(frame.current)
      frame.current = null
    }
    paintLive()
  }

  const onPointerMove = (e: React.PointerEvent<HTMLDivElement>) => {
    if (!dragging.current) return
    // `heightFromDrag` subtracts `dy`, because the handle is on the panel's TOP edge: dragging
    // down shrinks it. `check-toolwindow.mjs` asserts that sign, because a splitter with it
    // backwards resizes, clamps and commits perfectly while moving the wrong way.
    show(
      heightFromDrag(origin.current.height, e.clientY - origin.current.y, window.innerHeight, scale),
    )
  }

  /** Move now, tell the domain shortly — the keyboard half of the same bargain. */
  const moveTo = (height: number) => {
    if (!keying.current) {
      keying.current = true
      beginResizeGesture()
    }
    show(clampToolWindowHeight(height, window.innerHeight, scale))
    if (commitTimer.current !== null) window.clearTimeout(commitTimer.current)
    commitTimer.current = window.setTimeout(() => {
      commitTimer.current = null
      commit(live.current)
    }, KEY_COMMIT_DELAY)
  }

  /** Send a pending keyboard commit immediately, so none is ever dropped to a blur. */
  const flushCommit = () => {
    if (keying.current) {
      keying.current = false
      settle()
      endResizeGesture()
    }
    if (commitTimer.current === null) return
    window.clearTimeout(commitTimer.current)
    commitTimer.current = null
    commit(live.current)
  }

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    // Up grows and Down shrinks, matching the pointer: the panel's edge moves with the key.
    if (e.key === 'ArrowUp') moveTo(live.current + KEY_STEP)
    else if (e.key === 'ArrowDown') moveTo(live.current - KEY_STEP)
    else if (e.key === 'Home') moveTo(toolWindowCeiling(window.innerHeight, scale))
    else if (e.key === 'End') moveTo(0)
    else return
    e.preventDefault()
  }

  return (
    <div
      role="separator"
      aria-orientation="horizontal"
      aria-label="Resize the git tool window"
      aria-valuenow={painted(held)}
      aria-valuemin={TOOL_MIN}
      aria-valuemax={toolWindowCeiling(window.innerHeight, scale)}
      tabIndex={0}
      ref={selfRef}
      data-audit="toolWindowSplitter"
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
