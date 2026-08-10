/**
 * Notice when the IPC transport degrades, at any point in the window's life.
 *
 * # Why a boot probe is not enough
 *
 * `probeIpcOnce` runs from `App`'s mount effect and never again. But the thing it measures is
 * not a boot-time property: `tauri-2.11.5/scripts/ipc-protocol.js` sets
 * `customProtocolIpcFailed = true` from a rejection handler installed as the *second argument*
 * of the second `.then`, so it also fires on a `response.json()` / `arrayBuffer()` failure —
 * i.e. after the Rust command has already run — at any moment, hours in. From then on every
 * payload in the process is JSON-encoded and `eval`'d, permanently. Nothing crashes; the app
 * becomes slow.
 *
 * That is the exact shape of an unfalsifiable bug report ("it feels laggy"), and a boot-only
 * probe answers only the state nobody was asking about.
 *
 * # Why polling, and why this slowly
 *
 * There is no event for it — the flag is a module-local boolean inside Tauri's injected
 * script, and the only observable is a fetch of the `ipc:` scheme, which is what
 * `detectCustomProtocol` does. One zero-byte round trip every half minute is far below the
 * noise floor of anything the terminal does, and the failure it is watching for is permanent
 * once it happens, so there is nothing to be gained by looking more often.
 *
 * Reports only on **transition**, so the Rust log and the banner say "it just degraded" rather
 * than repeating the same line for the life of the window.
 */
import { diag } from './client'
import { detectCustomProtocol } from '@/bench/ipcBench'

/** How often to re-check. See the note above on why this is slow. */
export const TRANSPORT_POLL_MS = 30_000

/**
 * Whether the transport was healthy at the last check.
 *
 * `null` until the first one, so the first result is never reported as a transition — the boot
 * probe already reported it, and reporting a *change* that is really an initial reading would
 * make the banner appear on every launch on an already-degraded machine and say the wrong
 * thing about when it happened.
 */
let healthy: boolean | null = null
let timer: ReturnType<typeof setInterval> | null = null

/**
 * Start watching. Returns the stop function; calling twice is a no-op.
 *
 * `report` receives `false` when the transport has just degraded and `true` when it has
 * recovered — which it cannot, in Tauri's implementation, but the caller should not have to
 * know that and a one-way signal is harder to test.
 */
export function watchTransport(
  report: (customProtocol: boolean) => void,
  intervalMs: number = TRANSPORT_POLL_MS,
): () => void {
  if (timer !== null) return stopTransportWatch

  const tick = () => {
    void detectCustomProtocol().then(
      (ok) => {
        const previous = healthy
        healthy = ok
        if (previous !== null && previous !== ok) report(ok)
      },
      (e: unknown) => {
        // A probe that could not run is not evidence about the transport. Said once per
        // occurrence rather than swallowed, because a probe that has silently stopped working
        // is indistinguishable from a transport that never degrades.
        void diag.log(`ipc transport re-probe failed: ${String(e)}`).catch(() => {})
      },
    )
  }

  timer = setInterval(tick, intervalMs)
  tick()
  return stopTransportWatch
}

export function stopTransportWatch(): void {
  if (timer === null) return
  clearInterval(timer)
  timer = null
  healthy = null
}
