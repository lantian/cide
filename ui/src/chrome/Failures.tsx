/**
 * The surface that makes the outcome of a command visible.
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
 *
 * # Why successes are here too
 *
 * The same argument, one step further. `git.pull` had the *opposite* half of the bug: the
 * promise was fired with `void`, its rejection reached this stack, and its **resolved value
 * was discarded** — so a pull that failed said so and a pull that worked produced nothing at
 * all. A key that reports only its failures is a key the user believes is dead, which is the
 * exact symptom the paragraphs above were written about.
 *
 * So this renders `chrome/notices.ts`, which holds all three kinds. Two differences between
 * them, and no more: the colour of the edge, and the ARIA role. A modal treatment was rejected
 * for the same reason it was rejected for failures — see the head of `Failures.module.css`.
 */
import { useEffect, useSyncExternalStore } from 'react'
import {
  dismiss,
  getServerSnapshot,
  getSnapshot,
  notifyFailure,
  subscribe,
  type Notice,
} from './notices'
import { Icon } from '@/icons/Icon'

import styles from './Failures.module.css'

export function Failures(): React.ReactNode {
  const notices = useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot)

  useEffect(() => {
    const onRejection = (event: PromiseRejectionEvent) => {
      // Still logged. This surface is for the user; the console is for whoever is debugging,
      // and it carries the stack and the tagged variant that `describe` throws away.
      console.error('[cide] a command failed', event.reason)
      notifyFailure(event.reason)
    }
    window.addEventListener('unhandledrejection', onRejection)
    return () => window.removeEventListener('unhandledrejection', onRejection)
  }, [])

  if (notices.length === 0) return null

  return (
    <div className={styles.stack} data-audit="failures">
      {notices.map((notice) => (
        <Toast key={notice.id} notice={notice} />
      ))}
    </div>
  )
}

function Toast({ notice }: { notice: Notice }) {
  /*
   * Everything but `ok` interrupts. The full argument is at `NoticeKind` in `notices.ts`; the
   * short version is that a refusal is not a success — "there is no file path under the
   * pointer" answers a click the user has just made, and a screen-reader user who hears
   * nothing concludes the gesture is wired to nothing.
   */
  const loud = notice.kind !== 'ok'
  return (
    /*
     * The role is per toast, not on the stack, and that is an accessibility decision rather
     * than a tidiness one. `alert`/`assertive` is right for a failure and for a refusal: both
     * are the outcome of something the user just did and both have to interrupt a screen
     * reader rather than wait for a pause. It is wrong for a report — announcing
     * "fast-forwarded 7 commits" over the sentence someone is in the middle of reading is a
     * modal dialog for a success. The stack carried one role for everything while everything
     * was a failure; it cannot now.
     *
     * The kind reaches CSS as `data-kind` rather than as a class picked out of a table here.
     * Same reasoning `ChangesTree.module.css` writes out for its `data-status` selectors: a
     * rule whose selector *is* the vocabulary value fails **visibly** when the vocabulary
     * changes — the edge falls back to `--border` and reads grey — where a
     * `Record<NoticeKind, string>` whose entry has gone stale interpolates the literal string
     * "undefined" into `className` and paints nothing, which `tsc` cannot see either.
     */
    <div
      className={styles.toast}
      data-kind={notice.kind}
      role={loud ? 'alert' : 'status'}
      aria-live={loud ? 'assertive' : 'polite'}
      data-audit="noticeToast"
    >
      <div className={styles.body}>
        <p className={styles.text}>{notice.text}</p>
        {notice.hint !== undefined && <p className={styles.hint}>{notice.hint}</p>}
        {notice.detail !== undefined && notice.detail !== '' && (
          /*
           * A disclosure rather than always-on text. The headline answers "did it work and by
           * how much"; the body answers "what exactly", which is a question only some of the
           * time — and a toast that is twelve lines high by default covers the pane the user
           * pulled *for*.
           *
           * `<details>` rather than a button and a piece of state: it is the element for this,
           * it is keyboard-operable and announced as a disclosure with no ARIA of our own, and
           * the open state belongs to the toast rather than to anything that outlives it.
           */
          <details className={styles.more}>
            <summary className={styles.summary}>Details</summary>
            {/* Pre-wrapped, never parsed. On the `git` binary route this text includes the
                remote server's own `remote:` lines, which are written by whoever runs that
                server: it is displayed and nothing else. */}
            <pre className={styles.detail}>{notice.detail}</pre>
          </details>
        )}
      </div>
      <button
        type="button"
        className={styles.dismiss}
        title="Dismiss"
        aria-label="Dismiss"
        onClick={() => dismiss(notice.id)}
      >
        <Icon name="x" size={1} />
      </button>
    </div>
  )
}
