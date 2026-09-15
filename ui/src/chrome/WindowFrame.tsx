/**
 * Resize borders and double-click-to-maximize for an undecorated window.
 *
 * The window is built `decorations(false)` (ADR 0006) because the design draws its own
 * title bar. On Wayland that forfeits every service the window manager was providing for
 * free: resize borders, snap, double-click-to-maximize. This component hands them back by
 * covering the window's own perimeter with eight invisible grips.
 *
 * This file imports `@tauri-apps/api/window` directly, which no module except
 * `ipc/client.ts` is otherwise allowed to do. That rule governs the *command* surface —
 * the generated `invoke` wrappers whose wire types must stay in one place. Window
 * manipulation is not a command: routing it through the IPC client would mean inventing a
 * Rust handler that does nothing but call back into the same webview API. Please do not
 * "fix" this by adding one.
 *
 * There is deliberately no positioning here, matching the absent `.center()` in the Rust
 * builder: Wayland does not let a client place its own surface, so asking is ignored.
 */
import { useSyncExternalStore } from 'react'
import type { PointerEvent as ReactPointerEvent, ReactNode } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { guardUnlisten } from '@/ipc/unlisten'
import { currentUserAgent, drawsOwnResizeGrips } from './windowControls'
import styles from './WindowFrame.module.css'

/*
 * `ResizeDirection` is declared in the installed typings but not exported from the module,
 * so it is recovered from the method that consumes it. Derived rather than retyped by hand
 * so that a version bump which changes the union fails the typecheck instead of failing at
 * runtime with an unrecognised direction.
 */
type ResizeDirection = Parameters<ReturnType<typeof getCurrentWindow>['startResizeDragging']>[0]

/** CSS module lookups are `string | undefined` under `noUncheckedIndexedAccess`. */
function cls(name: string): string {
  return styles[name] ?? ''
}

interface Grip {
  direction: ResizeDirection
  /** Class name in the stylesheet, which owns the geometry and the cursor for each grip. */
  variant: string
  title: string
}

/*
 * Edges first, corners second: they are siblings in source order, and the corners carry the
 * higher z-index in the stylesheet. Without that a corner sits under the two edges that
 * cross it and the diagonal resize is unreachable — the 12x12 square would only ever
 * deliver `ns-resize` or `ew-resize`.
 */
const EDGES: readonly Grip[] = [
  { direction: 'North', variant: 'n', title: 'Resize top edge' },
  { direction: 'South', variant: 's', title: 'Resize bottom edge' },
  { direction: 'East', variant: 'e', title: 'Resize right edge' },
  { direction: 'West', variant: 'w', title: 'Resize left edge' },
]

const CORNERS: readonly Grip[] = [
  { direction: 'NorthWest', variant: 'nw', title: 'Resize top-left corner' },
  { direction: 'NorthEast', variant: 'ne', title: 'Resize top-right corner' },
  { direction: 'SouthWest', variant: 'sw', title: 'Resize bottom-left corner' },
  { direction: 'SouthEast', variant: 'se', title: 'Resize bottom-right corner' },
]

/* ------------------------------------------------------------------ maximized state --- */

/*
 * The maximized flag lives at module scope rather than in component state so that every
 * `useWindowChrome()` caller reads one value and the window keeps exactly one resize
 * listener and one delegated double-click listener no matter how many components subscribe.
 * Under StrictMode's deliberate double-mount that also means the listeners are torn down
 * and reattached once, not accumulated.
 */
let maximized = false
const subscribers = new Set<() => void>()
let unlistenResize: (() => void) | null = null
/* Bumped on every attach and detach so a slow `onResized` registration that lands after its
 * subscriber has gone can tell that it is stale and unlisten itself. */
let generation = 0

function emit(): void {
  for (const notify of subscribers) notify()
}

async function refresh(): Promise<void> {
  const next = await getCurrentWindow().isMaximized()
  if (next === maximized) return
  maximized = next
  emit()
}

/**
 * Maximize the window if it is restored, restore it if it is maximized.
 *
 * Stable identity across renders, so it is safe in a dependency array.
 *
 * This needs `core:window:allow-toggle-maximize` in every capability file, and it is a
 * *separate* permission from `allow-maximize` and `allow-unmaximize` — holding both of those
 * does not imply it. Without it the call is rejected in the webview with `Command
 * plugin:window|toggle_maximize not allowed by …`, which is how the zoom button and
 * double-click-to-maximize both shipped inert: Tauri's own injected drag script
 * (`src/window/scripts/drag.js`) invokes the same command for its double-click, so one missing
 * line took out both gestures at once and neither of them logs anywhere the user looks.
 */
export async function toggleMaximize(): Promise<void> {
  await getCurrentWindow().toggleMaximize()
  // The resize event covers this too, but the round trip is what the caller awaited, so the
  // flag should be settled by the time the promise resolves.
  await refresh()
}

/*
 * Tauri's injected drag script already toggles maximize on the second mousedown inside a
 * `data-tauri-drag-region` (tauri 2.11.5, src/window/scripts/drag.js, `e.detail === 2`), and
 * the header's drag filler carries that attribute alongside `data-window-drag`. Acting here
 * as well would toggle twice and leave the window exactly where it started, so this pair of
 * helpers mirrors that script's `isDragRegion` walk and stands down whenever it says yes.
 *
 * The rule is narrower than "an ancestor has the attribute", which is why it is copied
 * rather than approximated with `closest()`: a bare or `"true"` region only fires for a
 * direct hit on the region element itself, and a clickable element between the target and
 * the region blocks the gesture outright. Both cases are ones Tauri declines and this
 * listener therefore has to take.
 */
const TAURI_CLICKABLE_TAGS = new Set([
  'A',
  'BUTTON',
  'INPUT',
  'SELECT',
  'TEXTAREA',
  'LABEL',
  'SUMMARY',
])
const TAURI_INTERACTIVE_ROLES = new Set([
  'button',
  'link',
  'menuitem',
  'tab',
  'checkbox',
  'radio',
  'switch',
  'option',
])

function isTauriClickable(el: HTMLElement): boolean {
  return (
    TAURI_CLICKABLE_TAGS.has(el.tagName) ||
    (el.hasAttribute('contenteditable') && el.getAttribute('contenteditable') !== 'false') ||
    (el.hasAttribute('tabindex') && el.getAttribute('tabindex') !== '-1') ||
    TAURI_INTERACTIVE_ROLES.has(el.getAttribute('role') ?? '')
  )
}

function tauriOwnsDoubleClick(path: readonly EventTarget[]): boolean {
  const [innermost] = path
  for (const el of path) {
    if (!(el instanceof HTMLElement)) continue

    const attr = el.getAttribute('data-tauri-drag-region')
    if (attr === null) {
      if (isTauriClickable(el)) return false
      continue
    }
    if (attr === 'false') return false
    if (attr === 'deep') return true
    if (attr === '' || attr === 'true') return el === innermost
  }
  return false
}

function onDocumentDoubleClick(event: MouseEvent): void {
  const target = event.target
  if (!(target instanceof Element)) return

  const handle = target.closest('[data-window-drag="true"]')
  if (handle === null) return

  if (tauriOwnsDoubleClick(event.composedPath())) return

  void toggleMaximize()
}

/**
 * Close, minimize and zoom, delegated the same way the double-click handler is.
 *
 * The header draws the traffic lights but must not act on them: only `ipc/client.ts` may
 * import `@tauri-apps/api`, and window manipulation is not a command. So the header marks
 * each light with `data-window-button` and the behaviour lives here, with the rest of the
 * window handling. Without this the three most prominent controls in the app are focusable
 * buttons that do nothing.
 */
function onDocumentClick(event: MouseEvent): void {
  const target = event.target
  if (!(target instanceof Element)) return

  const button = target.closest('[data-window-button]')
  if (button === null) return

  const window = getCurrentWindow()
  switch (button.getAttribute('data-window-button')) {
    case 'close':
      void window.close()
      break
    case 'minimize':
      void window.minimize()
      break
    case 'zoom':
      void toggleMaximize()
      break
  }
}

function attach(): void {
  const mine = ++generation

  /*
   * Delegated on `document` rather than passed down as a prop: the drag filler belongs to
   * the header and the window belongs to this module, and neither should have to hold a
   * reference to the other to make a double-click work.
   */
  document.addEventListener('dblclick', onDocumentDoubleClick)
  document.addEventListener('click', onDocumentClick)

  void refresh()
  void getCurrentWindow()
    .onResized(() => {
      void refresh()
    })
    .then((fn) => {
      /*
       * Guarded, because this is the branch that loses the registration race: `attach` and
       * `detach` run back to back whenever the last subscriber goes and another arrives, and
       * `else unlisten()` then fires at the earliest instant the handle exists — which is
       * sometimes before the eval that registered it has run. See `ipc/unlisten.ts`; the
       * `.catch` below never saw this one, because the rejection is the discarded result of
       * the call rather than of the chain.
       */
      const unlisten = guardUnlisten(fn)
      if (mine === generation) unlistenResize = unlisten
      else void unlisten()
    })
    .catch(() => {
      /* No window event stream means the flag stays at whatever the one-shot poll above read
       * and only `toggleMaximize` moves it again. A maximize performed outside this app then
       * goes unnoticed, and — because the grips are removed while the flag says maximized —
       * a restore performed outside the app leaves them hidden until the next toggle. */
    })
}

function detach(): void {
  generation++
  document.removeEventListener('dblclick', onDocumentDoubleClick)
  document.removeEventListener('click', onDocumentClick)
  unlistenResize?.()
  unlistenResize = null
}

function subscribe(notify: () => void): () => void {
  subscribers.add(notify)
  if (subscribers.size === 1) attach()
  return () => {
    subscribers.delete(notify)
    if (subscribers.size === 0) detach()
  }
}

function getSnapshot(): boolean {
  return maximized
}

export interface WindowChrome {
  /** Whether the window is currently maximized, tracked through the window's resize event. */
  isMaximized: boolean
  toggleMaximize: () => Promise<void>
}

/**
 * Window-level state and actions that are not commands: the maximized flag and the toggle.
 *
 * The header's zoom button reads `isMaximized` to render the right affordance.
 */
export function useWindowChrome(): WindowChrome {
  const isMaximized = useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
  return { isMaximized, toggleMaximize }
}

/* ------------------------------------------------------------------------- the frame --- */

export interface WindowFrameProps {
  children: ReactNode
}

export function WindowFrame({ children }: WindowFrameProps) {
  const { isMaximized } = useWindowChrome()

  function beginResize(event: ReactPointerEvent<HTMLDivElement>, direction: ResizeDirection) {
    // Primary button only: a right-click on the border belongs to the window menu, not to a
    // resize the user cannot then cancel.
    if (event.button !== 0) return

    // Suppress the drag-to-select that would otherwise start under the compositor's grab.
    event.preventDefault()

    /*
     * Hand the gesture to the compositor and forget about it. Tracking pointermove and
     * calling `setSize` is the obvious alternative and it is wrong on Wayland: the client
     * does not own its surface geometry, so every frame arrives a configure late, which
     * reads as lag, and the compositor's own min/max and tiling constraints end up fighting
     * the numbers we send. Note that no pointerup follows — the grab leaves the webview —
     * which is exactly why nothing here holds per-gesture state.
     */
    void getCurrentWindow().startResizeDragging(direction)
  }

  /*
   * Grips are removed, not merely disabled, while maximized. Dragging the border of a
   * maximized window on KDE leaves it in a half-maximized state that reports maximized but
   * no longer fills the output, and nothing in the app can put it back.
   *
   * They are removed on macOS for the whole life of the window, for the reason
   * `drawsOwnResizeGrips` states at length: `startResizeDragging` is unimplemented there and
   * an undecorated NSWindow keeps its own edge resize, so drawing them takes a working gesture
   * away and gives nothing back. Read from the user agent rather than from a command, matching
   * the button layout beside it — the alternative costs an async round trip before the frame
   * can render, and a frame of grips that eat the pointer is exactly what is being removed.
   */
  const platformResizes = !drawsOwnResizeGrips(currentUserAgent())
  const grips = isMaximized || platformResizes ? [] : [...EDGES, ...CORNERS]

  return (
    <div className={cls('frame')}>
      {children}
      {grips.map((grip) => (
        /*
         * Not a <button>, despite the rule for the rest of this chrome. A grip is a pointer
         * gesture handed straight to the compositor; there is no keyboard equivalent to
         * fire, so making it focusable would add eight invisible tab stops that do nothing
         * when activated. It carries a title for pointer users and is hidden from the
         * accessibility tree instead.
         */
        <div
          key={grip.direction}
          className={`${cls('grip')} ${cls(grip.variant)}`}
          title={grip.title}
          aria-hidden="true"
          onPointerDown={(event) => beginResize(event, grip.direction)}
        />
      ))}
    </div>
  )
}
