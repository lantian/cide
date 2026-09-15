/**
 * Making `unlisten()` survive the one moment it is most likely to be called.
 *
 * # The failure
 *
 * A red toast reading **undefined is not an object (evaluating 'listeners[eventId].handlerId')**,
 * raised by `chrome/Failures.tsx` because nothing catches what `unlisten()` rejects with. It is
 * not a cide error at all: it is a `TypeError` thrown inside Tauri, and the reason it reaches
 * the user is that an unsubscribe is exactly the kind of call every site fires and forgets.
 *
 * # Why Tauri throws
 *
 * `listen()` is answered over **two transports that have no ordering between them**.
 *
 *   - The registration — `Object.defineProperty(eventListeners, '<id>', …)` — is a *script*.
 *     `Webview::listen_js` hands it to `webview.eval`, which queues it on the tao event loop
 *     for the main thread to pass to `evaluateJavaScript`.
 *   - The **answer** to `invoke('plugin:event|listen')`, which is that same id, comes back as
 *     the body of a `fetch('ipc://localhost/…')` (tauri's `scripts/ipc-protocol.js`, answered
 *     by `ipc/protocol.rs`'s `get()` responder, which calls `responder.respond` and never
 *     `eval`).
 *
 * So `await listen(…)` can resolve *before* the script that defines its id has run. Unsubscribe
 * in that window and Tauri's generated `unregisterListener` does:
 *
 *     const listeners = (window['…listeners_object_id…'] || {})[event]
 *     if (listeners) {
 *       window.__TAURI_INTERNALS__.unregisterCallback(listeners[eventId].handlerId)
 *     }
 *
 * The guard is on the **event-name bucket**, not on the id — and the bucket is there, created by
 * whichever earlier subscriber to the same event name won the race. `listeners[eventId]` is
 * `undefined`, and `.handlerId` throws. Tauri itself knows this shape: the emit script it
 * generates beside this one guards each lookup with `if (listener)`. This one was not given the
 * same guard.
 *
 * It is not only noise. The throw is on `_unlisten`'s **first** line, so the
 * `invoke('plugin:event|unlisten')` under it never runs: Rust keeps the listener, and the
 * handler goes live in the document as soon as the eval does land. A leaked subscription with
 * a toast attached — the project switch that raises the toast also leaves a stale tree
 * refresher behind, which is the shape `Explorer.tsx`'s `cancelled` flag exists to prevent.
 *
 * # Why a retry rather than a swallow
 *
 * Swallowing would silence the toast and keep the leak, which is the worse half of the bug and
 * the invisible one. The registration is *in flight* — it is a queued eval, not a lost one — so
 * the same handle works a frame later. `guardUnlisten` therefore tries again a few times over a
 * quarter of a second and only then gives up, quietly, to the console.
 *
 * Calling an unlisten twice is safe at the Tauri end — nothing deletes the entry that
 * `unregisterListener` reads — which is what makes a retry of a partly-succeeded composite
 * (`onDragDropEvent` removes four listeners in a row) correct rather than merely harmless.
 *
 * The returned function is idempotent and **never rejects**. That is the contract the call
 * sites want: `return () => unlisten?.()` in an effect cleanup has nowhere to put an error, and
 * `chrome/Failures.tsx` is right to show it everything it is handed.
 *
 * Deliberately import-free, so `check:unlisten` can compile and drive it standalone.
 */

/** What `listen()` hands back: a function that removes the subscription. */
export type Unlisten = () => void | Promise<void>

/**
 * The wait before each retry, in milliseconds.
 *
 * First retry on the next macrotask, because a queued eval usually only needs the current one
 * to finish; then a frame, then four, then a quarter of a second — five attempts in all. The
 * ceiling is deliberately short: past that the registration is not late, the webview is going
 * away, and there is nothing left to unsubscribe from.
 */
export const UNLISTEN_RETRY_MS: readonly number[] = [0, 16, 64, 256]

export interface GuardOptions {
  /** Injected by `check:unlisten` so the retry ladder can be driven without real time. */
  sleep?: (ms: number) => Promise<void>
  /** Where a handle that never came good is reported. Defaults to the console. */
  onGiveUp?: (error: unknown) => void
}

/**
 * Wrap an unlisten handle so it retries the registration race and can never reject.
 *
 * @param unlisten the handle `listen()` resolved with
 */
export function guardUnlisten(unlisten: Unlisten, options: GuardOptions = {}): () => Promise<void> {
  const sleep = options.sleep ?? ((ms) => new Promise<void>((resolve) => setTimeout(resolve, ms)))
  const giveUp = options.onGiveUp ?? reportToConsole

  /*
   * The first call's promise, reused by every later one.
   *
   * Idempotence is not tidiness here. A cleanup that runs while a retry is still pending —
   * `Explorer.tsx` unsubscribes on every project switch, and a user can switch twice inside
   * 256 ms — would otherwise start a second ladder against the same handle, and the two would
   * report the same give-up twice for one subscription.
   */
  let running: Promise<void> | null = null

  return () => {
    if (running !== null) return running
    running = (async () => {
      let last: unknown
      for (let attempt = 0; ; attempt += 1) {
        try {
          await unlisten()
          return
        } catch (error) {
          last = error
          if (attempt >= UNLISTEN_RETRY_MS.length) break
          await sleep(UNLISTEN_RETRY_MS[attempt] ?? 0)
        }
      }
      giveUp(last)
    })()
    return running
  }
}

/*
 * Not a toast. A subscription that could not be removed is a leak the user can do nothing
 * about and has no gesture to connect it to — `chrome/Failures.tsx` exists for the outcome of
 * something somebody just did. The console is for whoever is debugging, and it is where the
 * five failed attempts are worth reading.
 */
function reportToConsole(error: unknown): void {
  console.warn('[cide] an event subscription could not be removed', error)
}
