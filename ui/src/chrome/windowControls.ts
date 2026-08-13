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
