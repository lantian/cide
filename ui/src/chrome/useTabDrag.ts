/**
 * The pointer half of dragging a tab along the strip. Every *rule* it applies lives in
 * `tabDrag.ts`; this file is the mechanism, and it is the part no check in this repo can run.
 *
 * # Pointer events, not HTML5 drag and drop
 *
 * The third time this repository has settled the question, and the reasons are the ones
 * `sidebar/useTreeDrag.ts` and `sidebar/GitPanel/useChangesDrag.ts` already record, re-checked
 * against this surface rather than assumed:
 *
 * 1. **Tauri's native drag-drop handler is on.** `crates/cide-app/src/windows.rs` builds every
 *    window with `WebviewWindowBuilder::new(…)` and never calls `disable_drag_drop_handler()`,
 *    and `tauri.conf.json` has an empty `app.windows`, so there is no config route either.
 *    Turning it off is a global change this feature does not own, and it would kill OS file-drop
 *    onto the window everywhere. On WebKitGTK the swallowing of `dragstart`/`drop` is
 *    engine-dependent rather than the documented Windows certainty — which is an argument *for*
 *    pointer events, not against: a gesture that works in `pnpm dev` in a browser and does
 *    nothing in the shipped app is the worse failure, and it is the one this repo has shipped
 *    repeatedly.
 * 2. **`dataTransfer` buys nothing here.** The payload is one tab id that never leaves this
 *    component, and the drop position is arithmetic over the strip's own rects either way.
 * 3. **One drag mechanism in one app.** The splitters, the sidebar splitter, the editor minimap,
 *    the git panel and the file tree are all pointer capture plus window listeners.
 *
 * # What is deliberately **not** copied from the other two hooks
 *
 * **`edgeScroll`.** `TabStrip.module.css` gives `.tabs` `overflow: hidden` with `flex: none` on
 * every tab: the strip **clips**, it does not scroll, so there is no scroll container to nudge
 * and a copied `edgeScroll` would be a call on a box whose `scrollTop` is permanently 0. The
 * consequence, inherited rather than introduced: a tab clipped past the right edge cannot be a
 * drop target, because it is neither painted nor reachable. Making `.tabs` scrollable is a
 * larger, separate change — it costs the strip a scrollbar or a custom one, and it interacts
 * with the awaiting marker's reserved box, which the stylesheet measures at ~2 tabs of capacity.
 *
 * **`preventDefault()` on the press.** `ChangesTree` does it to stop a text selection;
 * `useTreeDrag` explains why copying it breaks focus there. Here it would break the tab's own
 * `<button>`: a press whose default is prevented never focuses it, and the strip's focus ring
 * (`.strip button:focus-visible`) is the only keyboard affordance the tabs have. Text selection
 * is already refused by the prefixed `user-select: none` on the body — `styles/tokens.css` and
 * `check:css-prefix` — so there is nothing left for it to buy.
 *
 * # Aiming: rects, not `elementFromPoint`
 *
 * The other two hooks hit-test with `document.elementFromPoint` because their rows are
 * virtualized and re-flatten mid-drag. This strip is a dozen stable elements in one box, and it
 * has a problem they do not: it is **30 pixels tall**. A user dragging a tab sideways drifts
 * vertically out of it constantly, and `elementFromPoint` would then answer with the editor
 * underneath — resolving to "no tab", which `caretIndex` reads as the end of the strip. The tab
 * would jump to the end whenever the hand wandered 20px low, which is the drag feeling broken.
 *
 * So the aim is computed from the tabs' own rects against the pointer's **x only**, exactly as
 * every tab strip that works does. Vertical position is ignored for the whole gesture, which is
 * also what makes the drop forgiving enough to be usable at 30px.
 */
import { useCallback, useEffect, useRef, useState } from 'react'
import { caretIndex, dropOutcome, grabTab, type DragTab, type TabDragSet } from './tabDrag'

/** Enough movement to mean "drag" rather than "click". The same 4px the splitters use. */
const THRESHOLD = 4

export interface TabDragState {
  drag: TabDragSet
  /** The boundary the drop would land at — the gap *before* this index. */
  caret: number
  /** Where to draw the caret, in pixels from the tablist's left edge. */
  caretX: number
}

export interface UseTabDragOptions {
  tabs: readonly DragTab[]
  /** The tablist box. The caret is positioned inside it, and the aim is read off its children. */
  container: React.RefObject<HTMLDivElement | null>
  /**
   * Commit the move. Absent — the chrome audit's fixture strip, a server render — makes the
   * strip inert rather than pretending: a drag that cannot land is a drag that should not start.
   */
  onReorder?: ((tab: string, before: string | null) => void) | undefined
}

export interface TabDrag {
  state: TabDragState | null
  /** Put on every tab row; it decides for itself whether that tab can start a drag. */
  onPointerDown: (e: React.PointerEvent, tab: DragTab) => void
  /**
   * Did the gesture that is ending right now turn into a drag?
   *
   * Read from the tab's `onClick`, which fires after `pointerup`: a press that became a drag
   * must not *also* activate the tab it just moved. A function rather than a field so it can be
   * read after the state has been cleared.
   */
  dragged: () => boolean
}

export function useTabDrag(options: UseTabDragOptions): TabDrag {
  const { tabs, container, onReorder } = options
  const [state, setState] = useState<TabDragState | null>(null)

  /*
   * The listeners run for the life of one gesture and must see the *current* strip: a
   * `cide://workspace-changed` can open, close or retarget a tab mid-drag — an agent's `openDiff`
   * is the ordinary way that happens — and a handler closed over the tabs as they were at
   * mousedown would compute its boundary against a strip that no longer exists. Refs rather than
   * dependencies, because re-subscribing window listeners mid-drag loses the capture.
   */
  const live = useRef({ tabs, onReorder })
  live.current = { tabs, onReorder }

  const gesture = useRef<{
    pointerId: number
    from: { x: number; y: number }
    tab: DragTab
    drag: TabDragSet | null
    /**
     * The verdict as of the last move.
     *
     * Kept here and not read back out of React state at drop time. A state updater is called
     * twice under StrictMode and may be replayed at will, so it is the wrong place to fire a
     * workspace mutation from — the trap `useGitPanel::runConfirm` documents — and this ref is
     * written synchronously by the move that produced it, so there is no flush to race.
     */
    outcome: { tab: string; before: string | null } | null
    stop: (() => void) | null
  } | null>(null)

  /** Whether the *last* gesture became a drag. Cleared by the next press, not by the drop. */
  const wasDrag = useRef(false)

  const finish = useCallback((commit: boolean) => {
    const active = gesture.current
    gesture.current = null
    active?.stop?.()
    setState(null)
    const outcome = active?.outcome
    if (!commit || outcome === undefined || outcome === null) return
    live.current.onReorder?.(outcome.tab, outcome.before)
  }, [])

  const onPointerDown = useCallback(
    (e: React.PointerEvent, tab: DragTab) => {
      // A second pointer while one is already down is not a new gesture, so it must not clear
      // the verdict of the one in flight — the drop's `click` is still to come.
      if (gesture.current !== null) return
      /*
       * A new press: whatever the previous gesture was, this one is a click until it moves.
       *
       * **Above the button and modifier guards, not below them.** `dragged()` is read from a
       * `click` that fires after the *next* press, so a press that returns early here without
       * clearing the flag leaves the previous drag's verdict standing — and the first ctrl+click
       * on a tab after any drag would be swallowed by a guard about a gesture that had already
       * ended.
       */
      wasDrag.current = false
      /*
       * Left button only, and never from a modified press. A modified click on a tab is not a
       * reorder gesture — and the middle button is a close in most editors, so claiming it here
       * would pre-empt a control this strip may yet grow.
       */
      if (e.button !== 0 || e.ctrlKey || e.metaKey || e.shiftKey || e.altKey) return
      /*
       * Nothing to land on, so nothing is picked up. `onReorder` is absent in the chrome audit's
       * fixture strip (`App.tsx` renders `<TabStrip tabs={AUDIT_TABS} …>` with no handlers), and
       * without this the drag would run in full — the tab dimmed, the caret tracking the pointer
       * — and the drop would do nothing whatever, because `finish` ends at an optional call.
       * That is the silent failure this feature exists to remove, dressed as the feature. Read
       * through `live` rather than closed over, so a strip that gains the action between renders
       * is not stuck inert until it remounts.
       */
      if (live.current.onReorder === undefined) return
      // The pinned console. Refused here as well as at the threshold below, so the press does not
      // even register a gesture — see `tabDrag.grabTab`.
      if (grabTab(live.current.tabs, tab) === null) return

      const el = e.currentTarget
      if (!(el instanceof HTMLElement)) return

      const move = (ev: PointerEvent) => {
        const active = gesture.current
        if (active === null || ev.pointerId !== active.pointerId) return
        if (active.drag === null) {
          const far =
            Math.abs(ev.clientX - active.from.x) > THRESHOLD
            || Math.abs(ev.clientY - active.from.y) > THRESHOLD
          if (!far) return
          // Grabbed again at the threshold rather than at the press: the strip may have been
          // rewritten in between, and this is the order the user can actually see.
          const started = grabTab(live.current.tabs, active.tab)
          if (started === null) {
            finish(false)
            return
          }
          active.drag = started
          wasDrag.current = true
          // Only once the press is a drag: capturing on mousedown would break the click that
          // activates a tab, which is by far the more common thing to do to one.
          try {
            el.setPointerCapture(ev.pointerId)
          } catch {
            // A pointer already released (a very fast click) throws here. The drag simply runs
            // without capture, which is still correct — the listeners are on the window.
          }
        }
        const drag = active.drag
        if (drag === null) return
        const box = container.current
        if (box === null) return

        const { overIndex, fraction } = aim(box, live.current.tabs, ev.clientX)
        const caret = caretIndex(live.current.tabs.length, overIndex, fraction)
        active.outcome = dropOutcome(live.current.tabs, drag, caret)
        setState({ drag, caret, caretX: caretOffset(box, caret) })
      }

      const up = (ev: PointerEvent) => {
        const active = gesture.current
        if (active === null || ev.pointerId !== active.pointerId) return
        finish(active.drag !== null)
      }
      const cancel = () => finish(false)
      const key = (ev: KeyboardEvent) => {
        if (ev.key !== 'Escape') return
        // Only while something is in flight, and it must not also close whatever overlay is
        // behind the strip — hence the capture-phase listener and the stop.
        if (gesture.current === null || gesture.current.drag === null) return
        ev.preventDefault()
        ev.stopPropagation()
        finish(false)
      }

      window.addEventListener('pointermove', move)
      window.addEventListener('pointerup', up)
      window.addEventListener('pointercancel', cancel)
      window.addEventListener('keydown', key, true)
      gesture.current = {
        pointerId: e.pointerId,
        from: { x: e.clientX, y: e.clientY },
        tab,
        drag: null,
        outcome: null,
        stop: () => {
          window.removeEventListener('pointermove', move)
          window.removeEventListener('pointerup', up)
          window.removeEventListener('pointercancel', cancel)
          window.removeEventListener('keydown', key, true)
          if (el.hasPointerCapture(e.pointerId)) el.releasePointerCapture(e.pointerId)
        },
      }
    },
    [container, finish],
  )

  // A strip unmounted mid-drag — the window closing, the project closing — must not leave four
  // window listeners behind, firing at a dead component.
  useEffect(() => () => finish(false), [finish])

  /*
   * One-shot: reading it clears it.
   *
   * `wasDrag` exists to swallow the `click` a drop synthesises, so releasing a tab in its new
   * position does not also activate it. It was cleared only in `onPointerDown`, which is fine for
   * the pointer — every click has one — and wrong for the keyboard, which has none. The drag
   * deliberately does not `preventDefault()` the press (see the note at the top of this file), so
   * focus stays on the tab; the user then presses Enter, `click` fires with no `pointerdown`
   * before it, `wasDrag` is still true from the drop, and the activation is eaten. It stayed eaten
   * until the next pointer press landed on a tab — a keyboard user who dragged once could not
   * activate a tab again by keyboard at all.
   *
   * Consuming on read fixes both without a second flag: the synthesised click that follows a drop
   * is always the first reader, and anything after it — a keypress, a later click — sees false.
   */
  const dragged = useCallback(() => {
    const was = wasDrag.current
    wasDrag.current = false
    return was
  }, [])

  return { state, onPointerDown, dragged }
}

/**
 * Which tab the pointer's x is over, and how far across it — the input `caretIndex` takes.
 *
 * Resolved through `data-tab-id` back into the `tabs` array rather than by DOM position, so a
 * strip mid-render, or one that ever grows a non-tab child, cannot silently shift every index by
 * one. That attribute is already on the row for the context menu's hit-testing, which is the
 * same reason the other two hooks reuse `data-row-path` and `data-change-path`.
 *
 * `overIndex: -1` for a pointer past the last tab, which is the spacer — most of the strip on a
 * project with two tabs open, so it is the common case rather than an edge one. A pointer to the
 * *left* of the first tab produces a negative fraction on tab 0, which `caretIndex` clamps.
 */
function aim(
  box: HTMLElement,
  tabs: readonly DragTab[],
  x: number,
): { overIndex: number; fraction: number } {
  for (const el of box.querySelectorAll<HTMLElement>('[data-tab-id]')) {
    const rect = el.getBoundingClientRect()
    if (x >= rect.right) continue
    const index = tabs.findIndex((t) => t.id === el.dataset['tabId'])
    if (index < 0) continue
    return { overIndex: index, fraction: rect.width === 0 ? 0 : (x - rect.left) / rect.width }
  }
  return { overIndex: -1, fraction: 0 }
}

/**
 * Where to draw the caret for a boundary, in pixels from the tablist's left edge.
 *
 * `offsetLeft` rather than `getBoundingClientRect`, because the caret is absolutely positioned
 * *inside* the tablist and `offsetLeft` is already relative to it — `.tabs` is the offset parent,
 * which is exactly what `position: relative` on it buys. A rect-based version would have to
 * subtract the container's own rect and would drift by the border in one theme.
 *
 * Shifted half a pixel left so the 2px rule sits centred in the strip's 1px `gap` rather than
 * covering the leading edge of the tab after it.
 */
function caretOffset(box: HTMLElement, boundary: number): number {
  const els = box.querySelectorAll<HTMLElement>('[data-tab-id]')
  const at = els[boundary]
  if (at !== undefined) return Math.max(0, at.offsetLeft - 1)
  const last = els[els.length - 1]
  return last === undefined ? 0 : last.offsetLeft + last.offsetWidth
}
