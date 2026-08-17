/**
 * Which side of the title bar the window buttons sit on, and in what order.
 *
 * The window is undecorated (ADR 0006), so the header draws close/minimize/zoom itself. Where
 * they belong is a platform convention, not a taste: macOS puts them at the leading edge in
 * close-minimize-zoom order, and every other desktop puts them at the trailing edge with close
 * *outermost* — minimize-maximize-close. A macOS-ordered cluster on the right would put close
 * next to the neighbouring header action instead of hard against the window corner, which is
 * the one position a user aims at without looking. So side and order move together; they are
 * one decision and this module returns both.
 *
 * # Why the user agent, and not the OS plugin
 *
 * `@tauri-apps/plugin-os` would answer this authoritatively, but only `ipc/client.ts` may
 * import `@tauri-apps/api` (`WindowFrame.tsx` is the one documented exception, and adding a
 * second is how a rule stops being one). The alternative — a `#[tauri::command]` that returns
 * a compile-time constant — drifts `contract/commands.json` and costs an async round trip
 * before the header can lay itself out, which is a frame of the buttons in the wrong place on
 * every launch. The webview's user agent already carries the answer synchronously and is set
 * by the platform's own web view, so that is what is read.
 *
 * DOM-free and import-free on purpose: `scripts/check-window-controls.mjs` compiles this file
 * standalone and asserts on the table below. Keep it that way — the caller passes the user
 * agent in.
 */

/** The three buttons, named as `WindowFrame.tsx`'s delegated click handler knows them. */
export type WindowButtonId = 'close' | 'minimize' | 'zoom'

export type WindowControlSide = 'left' | 'right'

export interface WindowControlLayout {
  side: WindowControlSide
  /** Left-to-right paint order, whichever side they are on. */
  order: readonly WindowButtonId[]
}

/** macOS: leading edge, close first — the traffic lights as Apple orders them. */
const MAC: WindowControlLayout = { side: 'left', order: ['close', 'minimize', 'zoom'] }

/** Everywhere else: trailing edge, close outermost so it is the corner of the screen. */
const OTHER: WindowControlLayout = { side: 'right', order: ['minimize', 'zoom', 'close'] }

/**
 * True when this user agent came from a web view running on macOS.
 *
 * Both WKWebView and every desktop browser on macOS put `Macintosh` in the platform token;
 * iOS does not (it says `iPhone`/`iPad`), and cide has no iOS target, so the narrow token is
 * checked rather than a loose `mac` substring that also matches nothing useful. An unrecognised
 * or empty agent is not macOS — the app is Linux-first, so that is the right way to be wrong.
 */
export function isMacUserAgent(userAgent: string): boolean {
  return userAgent.includes('Macintosh')
}

/** Side and paint order for the window buttons on the platform this user agent describes. */
export function windowControlLayout(userAgent: string): WindowControlLayout {
  return isMacUserAgent(userAgent) ? MAC : OTHER
}

/**
 * Whether the frame paints its own eight resize grips, or leaves the edges to the OS.
 *
 * `WindowFrame.tsx` covers the window's perimeter with invisible grips and hands each
 * pointer-down to `startResizeDragging`, because on Wayland an undecorated window forfeits the
 * resize borders the compositor was providing. That is right on Linux and **wrong on macOS**,
 * for two separate reasons that happen to point the same way:
 *
 *  1. `startResizeDragging` does not exist there. `tao`'s macOS `drag_resize_window` is
 *     `Err(ExternalError::NotSupported)` unconditionally (tao-0.35.3
 *     `src/platform_impl/macos/window.rs`), and `WindowFrame` `void`s the rejected promise —
 *     so the grip resizes nothing and reports nothing.
 *  2. There is nothing to hand back in the first place. An undecorated window on macOS is
 *     `NSWindowStyleMask::Borderless | Resizable`, and AppKit keeps the edge resize for a
 *     borderless resizable window. The service Wayland takes away is one macOS never took.
 *
 * Painting them anyway is worse than useless rather than merely useless: the handler calls
 * `preventDefault()` before the doomed call, so the grip **swallows** a pointer-down that AppKit
 * would otherwise have turned into a resize. Eight invisible strips around the window that stop
 * the window being resized is the exact shape of a bug nobody would think to look for.
 *
 * # Why this is a function here and not a condition in the component
 *
 * Because a rule inside a React component is a rule no check script can compile. This module is
 * import-free and DOM-free precisely so `scripts/check-window-controls.mjs` can run it, and this
 * project has paid for the other arrangement repeatedly. The component reads the answer; it does
 * not contain it.
 *
 * Windows is deliberately not special-cased. WebView2 windows built `decorations(false)` are in
 * the same position as Linux's — `drag_resize_window` is implemented there — so the fallback
 * branch is right for both, and is also right for the non-browser runtimes (`''`) a check script
 * and an SSR bundle produce.
 */
export function drawsOwnResizeGrips(userAgent: string): boolean {
  return !isMacUserAgent(userAgent)
}

/**
 * The running webview's user agent, or `''` where there is no navigator.
 *
 * Guarded because the header is imported outside a browser: an SSR bundle, or a check script.
 * Both non-browser paths resolve to the non-mac layout and are indistinguishable from Linux —
 * `''` on a runtime with no `navigator`, and node's own `Node.js/22` on one that has it — which
 * is the right way to be wrong for a project that develops on Linux.
 */
export function currentUserAgent(): string {
  return typeof navigator === 'undefined' ? '' : navigator.userAgent
}
