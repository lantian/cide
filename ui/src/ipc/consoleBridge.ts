/**
 * Forward the webview's console into the Rust log.
 *
 * # Why this exists
 *
 * There is no way to read this app's console. It is a WebKitGTK webview on Wayland: there is
 * no terminal attached to it, `console.log` goes to a place nothing can reach, and opening
 * devtools needs a GUI window on a machine where the GUI is the thing being debugged. That
 * gap is not academic — it is why a keystroke-duplication bug took three attempts to fix, and
 * why `cmd/diag.rs` exists at all.
 *
 * `WEBKIT_INSPECTOR_SERVER` gives a real remote inspector and is the better tool when a real
 * inspector is what you need. This is the cheap half that covers the common case: something
 * threw, or warned, and nobody was listening. One `diag_log` per call, into the file
 * `tauri-plugin-log` already writes.
 *
 * # Why it is opt-in
 *
 * `diag_log` is a synchronous command, so every forwarded line is a main-thread IPC round trip
 * and a file append. A component that warns in a render loop would put that on the same thread
 * as the keystrokes. The same reasoning is written on the input probe in `windows.rs`, and for
 * the same reason: a diagnostic for a lag report must not itself be a cause of lag.
 *
 * `error` and `warn` only. `console.log` and `console.debug` are where this app narrates
 * itself — the terminal pane alone can emit several per keystroke — and forwarding them would
 * drown the signal in the file that is supposed to carry it.
 */
import { diag } from './client'

/** Set by `CIDE_CONSOLE_BRIDGE=1`, which `windows.rs` turns into a URL parameter. */
function bridgeRequested(): boolean {
  try {
    return new URLSearchParams(window.location.search).get('console') === '1'
  } catch {
    return false
  }
}

/**
 * Flatten what was logged into one line.
 *
 * An `Error` argument is the interesting case and the one `String(x)` ruins: it renders as
 * `"Error: message"` and drops the stack, which is the only part that says where it came from.
 * Everything else goes through `JSON.stringify`, falling back to `String` for the values that
 * cannot (circular objects, DOM nodes, functions).
 */
function render(args: readonly unknown[]): string {
  return args
    .map((a) => {
      if (a instanceof Error) return `${a.name}: ${a.message}\n${a.stack ?? '(no stack)'}`
      if (typeof a === 'string') return a
      try {
        return JSON.stringify(a)
      } catch {
        return String(a)
      }
    })
    .join(' ')
}

let installed = false

/**
 * Install the bridge. Idempotent, and a no-op unless asked for.
 *
 * Returns nothing to uninstall with on purpose: this is a launch-time switch, it lives for the
 * window's lifetime, and a restore function would be one more thing that could put the original
 * `console` back while a later caller still holds the patched one.
 */
export function installConsoleBridge(): void {
  if (installed || !bridgeRequested()) return
  installed = true

  for (const level of ['error', 'warn'] as const) {
    const original = console[level].bind(console)
    console[level] = (...args: unknown[]) => {
      original(...args)
      // Never `await`, and never let a failed log become a second failure: the bridge is the
      // thing reporting problems, so it must not be able to raise one of its own. A rejection
      // here would also reach `Failures` and put a toast on screen for a logging hiccup.
      void diag.log(`[console.${level}] ${render(args)}`).catch(() => {})
    }
  }

  // An uncaught error never reaches `console.error` in every browser, and an unhandled
  // rejection reaches it in none. `Failures` already shows the rejection to the user; this
  // writes it down where it can be read after the fact.
  window.addEventListener('error', (e) => {
    void diag.log(`[uncaught] ${e.message} @ ${e.filename}:${e.lineno}:${e.colno}`).catch(() => {})
  })
  window.addEventListener('unhandledrejection', (e) => {
    void diag.log(`[unhandled rejection] ${render([e.reason])}`).catch(() => {})
  })

  void diag.log('[console bridge] installed').catch(() => {})
}
