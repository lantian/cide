/**
 * Tells the user when the IPC transport has fallen back to `postMessage`.
 *
 * # Why this is on screen and not in the log
 *
 * The degradation is silent, permanent and invisible from inside the app: `ipc-protocol.js`
 * catches one fetch rejection, sets `customProtocolIpcFailed = true` for the life of the page,
 * and from then on every payload — including every PTY frame — is a JSON array of decimal
 * numbers spelled into a `webview.eval`. Nothing throws. The only symptom is that the app feels
 * slow, which is exactly the report that costs the most to diagnose, because it is
 * indistinguishable from a renderer problem, a PTY problem or a slow machine.
 *
 * It is not in the console either. `cmd/diag.rs` writes down why: the webview console is not
 * reachable from a shell on Wayland, and the devtools window is as hard to raise as the app
 * window. A diagnostic nobody can read is the same as no diagnostic — which is what this was
 * for the whole of M0 through M10, when `cide://ipc-degraded` was promised in a doc comment and
 * never existed in `contract/events.json`.
 *
 * Self-contained on purpose: `App` renders `<TransportNotice />` and passes nothing. The
 * subscription, the re-probe and the dismissal all live here, so the orchestrator's file gains
 * one element and no state.
 */
import { useEffect, useState } from 'react'
import { diag, onIpcDegraded, type IpcHealth } from './client'
import { watchTransport } from './transportWatch'
import styles from './TransportNotice.module.css'

export function TransportNotice() {
  const [health, setHealth] = useState<IpcHealth | null>(null)
  const [dismissed, setDismissed] = useState(false)

  useEffect(() => {
    let live = true
    let unlisten: (() => void) | null = null

    void onIpcDegraded((h) => {
      if (!live) return
      // A later degradation is worth showing again even if the user dismissed an earlier one:
      // the second one is news, and it is the one they will be typing through.
      setDismissed(false)
      setHealth(h)
    })
      .then((fn) => {
        if (live) unlisten = fn
        else fn()
      })
      .catch((e: unknown) => {
        void diag.log(`cannot listen for ipc-degraded: ${String(e)}`).catch(() => {})
      })

    // The boot probe answers once, at the only moment the transport is certain to still be
    // healthy. This is what notices the other case — see `transportWatch.ts`.
    const stop = watchTransport((customProtocol) => {
      if (customProtocol) return
      void diag
        .reportIpc({
          customProtocol: false,
          // Not measured here. Re-running the throughput benchmark on a slow timer would put
          // 24 KiB of round trips through a transport we have just decided is congested; the
          // figure that matters is on the boot report, and this event's job is to say *when*.
          mibPerSec: 0,
          webkitVersion: /AppleWebKit\/([\d.]+)/.exec(navigator.userAgent)?.[1] ?? 'unknown',
        })
        .catch(() => {})
    })

    return () => {
      live = false
      unlisten?.()
      stop()
    }
  }, [])

  if (health === null || dismissed) return null

  return (
    <div className={styles.strip} role="status">
      <span className={styles.mark}>!</span>
      <div className={styles.body}>
        cide is on the slow IPC path — every payload is being JSON-encoded and evaluated.
        Terminal output and typing will feel sluggish until this window is reloaded.{' '}
        <span className={styles.detail}>
          WebKit {health.webkitVersion}
          {health.mibPerSec > 0 ? ` · ${health.mibPerSec.toFixed(1)} MiB/s` : ''}
        </span>
      </div>
      <button type="button" className={styles.dismiss} onClick={() => setDismissed(true)}>
        Dismiss
      </button>
    </div>
  )
}
