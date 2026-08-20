/**
 * "A resize gesture is in flight" — the one fact every size-reactive observer in this window
 * needs, and the queue that lets them act on it.
 *
 * # The problem this exists for
 *
 * The three splitters (`layout/Splitter.tsx`, `chrome/SidebarSplitter.tsx`,
 * `toolwindow/ToolWindowSplitter.tsx`) are already careful: a drag writes CSS directly and
 * touches neither React state nor IPC between `pointerdown` and `pointerup`. The lag was never
 * in the gesture — it was in what *reacts* to the box actually changing size, and every one of
 * those reactions is expensive:
 *
 * * `PaneSlot`'s `ResizeObserver` calls `TerminalPane`'s `syncSize`, which calls
 *   `FitAddon.fit()` **unconditionally**. `fit()` reads computed style and a rect, and when the
 *   proposed cell count differs it calls `term.resize()` — a buffer reflow over `scrollback:
 *   5000`, rebuilt through the **DOM renderer**, which is this app's default (see
 *   `crates/cide-app/src/windows.rs`, where the WebGL addon's defect is written down). The
 *   `host.lastGeometry` guard suppresses only the `session_resize` that follows, one step late.
 * * `layout/TabContent.module.css` lays *every* tab's panes out at full size and hides the
 *   inactive ones with `visibility`, deliberately — so the cost above multiplies by tab count,
 *   up to `HOST_CAP` live terminals, for a sidebar or tool-window drag.
 * * `session_resize` is synchronous by design and reflows the vt100 scrollback on the thread
 *   that receives IPC messages, in front of the user's keystrokes (`cmd/session.rs`).
 * * `chrome/TabStrip.tsx` re-measures every tab's box, and `editor/minimap.ts` repaints a
 *   canvas, once per frame per editor.
 *
 * WebKitGTK does not coalesce `pointermove` to frames, so all of that was paid at the mouse's
 * report rate — 125 Hz on ordinary hardware — not at 60.
 *
 * # What this module decides
 *
 * **The terminal grid does not follow the drag.** Between `pointerdown` and `pointerup` a
 * terminal keeps the cell geometry it had; the pane box moves around it (the host is
 * `overflow: hidden`, so a shrink clips and a grow shows panel behind), and one refit plus one
 * `session_resize` per pane happens on release. The alternative — reflowing live, throttled —
 * cannot be made smooth, because every intermediate refit costs the child a full TUI repaint
 * and that repaint arrives as a screenful of bytes to render.
 *
 * Import-free on purpose: `ui/scripts/check-resize.mjs` compiles this one file standalone with
 * the TypeScript in `node_modules` and executes it, the same arrangement `chrome/sidebarWidth.ts`
 * and `toolwindow/toolWindowHeight.ts` have. Everything below is therefore reachable from a test
 * with no DOM — which is why `requestAnimationFrame` is looked up per call rather than captured
 * at module scope, and why `window` is never assumed to exist.
 */

/**
 * How many gestures are in flight.
 *
 * A count and not a boolean: a window manager can resize the window while a splitter is held,
 * and `noteExternalResize` and a splitter drag would then interleave. With a boolean the first
 * to finish would flush the other's deferred work in the middle of it, which is precisely the
 * per-frame refit this module exists to prevent.
 */
let depth = 0

/**
 * Work waiting for the gesture to end, one entry per key.
 *
 * A `Map` keyed on `unknown` so an **object identity** is a legal key: a component instance can
 * pass a `useRef({})` and needs no name that could collide with another instance's. Keyed at all
 * because a pane defers on every observer callback — two hundred times in one drag — and must
 * run once.
 */
const pending = new Map<unknown, () => void>()

/** The scheduled flush, so a second `endResizeGesture` does not queue a second one. */
let flushFrame: number | null = null

/** `noteExternalResize`'s trailing timer: the gesture nobody explicitly ends. */
let externalTimer: ReturnType<typeof setTimeout> | null = null

/** The watchdog. See [`beginResizeGesture`]. */
let watchdog: ReturnType<typeof setTimeout> | null = null

/**
 * How long after the last `resize` event a window drag is considered over.
 *
 * The OS window edge has no `pointerup` this window ever sees — the compositor owns the gesture
 * (`chrome/WindowFrame.tsx` hands it over with `startResizeDragging` and deliberately keeps no
 * per-gesture state) — so the only end available is a quiet period. 120 ms is longer than the
 * gap between two frames of a drag and short enough that letting go feels immediate.
 */
export const EXTERNAL_SETTLE_MS = 120

/**
 * The longest a gesture may be believed, in ms.
 *
 * A `depth` that never comes back to zero is the one failure mode of this design that is worse
 * than the bug it fixes: every terminal in the window would stop refitting for the rest of the
 * session, silently. Each caller pairs its `begin` with an `end` in `stop()` *and* in an unmount
 * effect, and this is the third net under those two. Deliberately much longer than any real
 * drag, so tripping it means something leaked rather than that someone dragged slowly.
 */
export const GESTURE_WATCHDOG_MS = 8000

/** Whether anything is currently dragging a boundary that changes panel sizes. */
export function isResizeGesturing(): boolean {
  return depth > 0
}

/**
 * Open a gesture. Every call must be paired with [`endResizeGesture`].
 *
 * The watchdog is armed on the *outermost* begin and re-armed by nested ones, so a long drag
 * that keeps reporting cannot trip it — only a gesture that stops talking can.
 */
export function beginResizeGesture(): void {
  depth += 1
  if (watchdog !== null) clearTimeout(watchdog)
  watchdog = setTimeout(() => {
    watchdog = null
    // Not a decrement: the point is to recover from a `depth` nobody is going to return.
    depth = 0
    flush()
  }, GESTURE_WATCHDOG_MS)
}

/** Close a gesture, flushing everything deferred once the last one closes. */
export function endResizeGesture(): void {
  if (depth === 0) return
  depth -= 1
  if (depth > 0) return
  if (watchdog !== null) {
    clearTimeout(watchdog)
    watchdog = null
  }
  scheduleFlush()
}

/**
 * Run `run` now, or once the gesture in flight ends.
 *
 * Runs **immediately** when nothing is dragging, so the common case — a pane mounting, a window
 * being tiled by the compositor, a font change — gains no latency at all from this module.
 *
 * A second call with the same key while deferred *replaces* the callback rather than adding one:
 * the caller is re-deferring the same work with a fresher closure, and running the stale one too
 * would be the duplicate refit this exists to remove.
 */
export function whenResizeSettles(key: unknown, run: () => void): void {
  if (depth === 0) {
    run()
    return
  }
  pending.set(key, run)
}

/**
 * Drop `key`'s deferred work.
 *
 * **Load-bearing on unmount, not tidiness.** `PaneSlot` defers `TerminalPane`'s `syncSize`, and
 * `syncSize` calls `getHost`, which hands an evicted pane its session back and builds a host for
 * it. Flushing after the slot has gone would therefore resurrect a host for a pane that is no
 * longer in the tree.
 */
export function cancelResizeSettle(key: unknown): void {
  pending.delete(key)
}

/**
 * Treat a burst of window `resize` events as one gesture.
 *
 * Called from the listener below and exported for the check script, which has no window to
 * resize.
 */
export function noteExternalResize(): void {
  if (externalTimer === null) beginResizeGesture()
  else clearTimeout(externalTimer)
  externalTimer = setTimeout(() => {
    externalTimer = null
    endResizeGesture()
  }, EXTERNAL_SETTLE_MS)
}

/**
 * Run the deferred work on the next frame.
 *
 * On a frame rather than inline so the browser paints the divider where the user let go
 * *before* the terminals reflow: the settle is then something that happens after the gesture
 * rather than something that delays its last frame. Falls back to running inline where there is
 * no `requestAnimationFrame` — a check script under node, an SSR bundle — because a queue that
 * silently never drains there would make this module untestable.
 */
function scheduleFlush(): void {
  if (pending.size === 0) return
  if (flushFrame !== null) return
  if (typeof requestAnimationFrame !== 'function') {
    flush()
    return
  }
  flushFrame = requestAnimationFrame(() => {
    flushFrame = null
    flush()
  })
}

function flush(): void {
  if (depth > 0) return
  // Drained before the first callback runs, not after the last: a callback that defers again —
  // a refit that changes a box and re-enters the observer it came from — must queue for the
  // *next* gesture rather than be dropped by the drain that is already in progress.
  const work = Array.from(pending.values())
  pending.clear()
  for (const run of work) {
    try {
      run()
    } catch (error) {
      // One pane's refit failing must not strand the other eleven. Reported and not rethrown for
      // the reason `TerminalPane` reports a failed resize: the pane is left at a size it is not,
      // which is visible, and taking the flush down with it would leave every other pane there
      // too.
      console.error('[cide] deferred resize work failed', error)
    }
  }
}

/*
 * The OS window's own resize, folded into the same mechanism.
 *
 * At module scope rather than in an effect, for the reason `chrome/SidebarSplitter.tsx` gives
 * for its own: this is window-lifetime, StrictMode's double-mount cannot install it twice, and
 * no unmount can tear it down while the window is still open.
 *
 * The guard tests `addEventListener` and not merely `window`, and that is not belt and braces.
 * `ui/scripts/check-rows.mjs` and `check-diff-render.mjs` SSR-bundle their entry through Vite and
 * run it under node against a **stub** `window` — an object with the handful of properties those
 * fixtures need and nothing else. `typeof window !== 'undefined'` is true there, and the call
 * then throws at module-evaluation time, taking down a check that has nothing to do with
 * resizing. Ask for the method that is about to be called.
 */
if (typeof window !== 'undefined' && typeof window.addEventListener === 'function') {
  window.addEventListener('resize', noteExternalResize)
}
