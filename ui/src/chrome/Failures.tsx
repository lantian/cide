/**
 * The surface that makes a failed command visible.
 *
 * # Why this exists
 *
 * Every mutation in `App.tsx` is fired as `void someMutation(…)` — 35 of the 40 call sites
 * have no `.catch`. That is a reasonable shape for a handler that cannot usefully recover, and
 * it has one catastrophic property: **a command that fails looks exactly like a control that
 * is wired to nothing.** The button depresses, the promise rejects, the rejection lands on the
 * floor, and the user is left to conclude the feature does not exist.
 *
 * That is not hypothetical. It is how the `+ row` buttons presented: they had been wired
 * correctly end to end, but `run.sh` hot-reloads the frontend through Vite while launching a
 * *pre-built* Rust binary, so the running process had no `pane_add_row` command. `invoke`
 * rejected with "Command pane_add_row not found", `void` discarded it, and the only symptom
 * was a dead button. The same shape had already cost a search button and a row of project
 * tabs, both of which really were unwired — and the point is that from the outside those three
 * were indistinguishable.
 *
 * # Why a window listener rather than 35 `.catch` calls
 *
 * `void p` on a rejecting promise still fires `unhandledrejection`. One listener therefore
 * covers every existing call site, every call site added later, and the ones in the store and
 * the panes that this file has never heard of — with no churn and nothing to remember. A
 * handler that *does* catch never reaches here, so the paths that answer an error properly
 * (`CloseConfirm` on `UnsavedChanges`, the picker's `NoIndex`) are unaffected by construction.
 *
 * The alternative that lost was reporting from inside `invoke` in `ipc/client.ts`. It sounds
 * more precise and is worse: it cannot tell a failure the caller is about to handle from one
 * nobody will, so every expected `UnsavedChanges` would raise a toast a half-frame before the
 * dialog that exists to explain it.
 */
import { useEffect, useState } from 'react'
import styles from './Failures.module.css'

interface Failure {
  id: number
  text: string
  /** Set when the message is one a user can act on themselves. */
  hint?: string | undefined
}

/**
 * Turn whatever was thrown into a sentence.
 *
 * Errors cross the IPC boundary as `{ kind, message }` — tagged, never bare strings, so the
 * frontend branches on a variant rather than matching prose. `message` is the half written for
 * a person; `kind` is for code and is deliberately not shown.
 */
function describe(reason: unknown): string {
  if (typeof reason === 'string') return reason
  if (reason instanceof Error) return reason.message
  if (reason !== null && typeof reason === 'object') {
    const message = (reason as { message?: unknown }).message
    if (typeof message === 'string' && message.length > 0) return message
    const kind = (reason as { kind?: unknown }).kind
    if (typeof kind === 'string') return kind
  }
  return String(reason)
}

/**
 * The one failure a user can fix without us, so it is worth naming precisely.
 *
 * Tauri answers an unrecognised command with this, and in development it means one specific
 * thing: the webview has hot-reloaded past the binary. Vite serves the new frontend instantly;
 * the Rust side only changes when it is rebuilt and the app is restarted. Without this hint the
 * message reads as an internal error rather than as "your binary is stale".
 */
function hintFor(text: string): string | undefined {
  return /not found|unknown command/i.test(text)
    ? 'The running binary predates this command — rebuild with `cargo build -p cide-app` and restart.'
    : undefined
}

/** How many are shown at once. Beyond this the oldest is dropped. */
const MAX_SHOWN = 3

export function Failures(): React.ReactNode {
  const [failures, setFailures] = useState<Failure[]>([])

  useEffect(() => {
    let next = 0
    const onRejection = (event: PromiseRejectionEvent) => {
      const text = describe(event.reason)
      // Still logged. This surface is for the user; the console is for whoever is debugging,
      // and it carries the stack and the tagged variant that `describe` throws away.
      console.error('[cide] a command failed', event.reason)
      setFailures((current) => {
        // Collapsed by message rather than appended. A failure that repeats is usually one
        // gesture the user is retrying, and three identical toasts explain no more than one.
        if (current.some((f) => f.text === text)) return current
        next += 1
        const failure: Failure = { id: next, text, hint: hintFor(text) }
        return [...current, failure].slice(-MAX_SHOWN)
      })
    }
    window.addEventListener('unhandledrejection', onRejection)
    return () => window.removeEventListener('unhandledrejection', onRejection)
  }, [])

  if (failures.length === 0) return null

  return (
    // `alert`, not `status`: this is the outcome of something the user just did, and it has
    // to interrupt a screen reader rather than wait for a pause.
    <div className={styles.stack} role="alert" aria-live="assertive" data-audit="failures">
      {failures.map((failure) => (
        <div key={failure.id} className={styles.toast}>
          <div className={styles.body}>
            <p className={styles.text}>{failure.text}</p>
            {failure.hint !== undefined && <p className={styles.hint}>{failure.hint}</p>}
          </div>
          <button
            type="button"
            className={styles.dismiss}
            title="Dismiss"
            aria-label="Dismiss"
            onClick={() => setFailures((c) => c.filter((f) => f.id !== failure.id))}
          >
            ×
          </button>
        </div>
      ))}
    </div>
  )
}
