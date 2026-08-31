/**
 * The whole event behind one rendered log line.
 *
 * A shell pane draws a structured log line as a one-line summary — `cide_core::jsonlog` — and
 * the summary is lossy in exactly one direction: a nested object arrives as compact JSON and a
 * wide event wraps. Clicking the timestamp opens this, which is the only way back to the object,
 * because the rewrite happens above the screen mirror and the raw bytes are in no buffer at all.
 * `cide-app`'s `logring` is where they are kept, and Rust does the pretty-printing so a detached
 * pane — a separate JavaScript realm — cannot format the same event differently from a docked one.
 *
 * Mounted in **both** branches of `App.tsx`, beside `<OutsideOpenGate/>`, for that component's
 * reason: a torn-out pane renders log lines exactly as a docked one does, and a card wired only
 * into the shell tree would leave the click in it doing nothing.
 *
 * The JSON is coloured with the editor's own grammar, reached through a dynamic `import()` the
 * way `panes/diffHighlight.ts` reaches it, and degrades to plain text if that load fails — a
 * viewer that shows nothing because a tokenizer would not load is worse than an uncoloured one.
 */
import { useEffect, useRef, useState } from 'react'
import { OverlayCard } from '@/overlays/ModalShell'
import { copyText } from '@/sidebar/copyText'
import { notify } from './notices'
import { useLogDetail } from './logDetailStore'
import styles from './LogDetailCard.module.css'

interface Token {
  readonly text: string
  readonly cls: string | null
}

export function LogDetailCard() {
  const pending = useLogDetail((s) => s.pending)
  const [lines, setLines] = useState<Token[][] | null>(null)
  const closeRef = useRef<HTMLButtonElement>(null)
  const pretty = pending?.detail?.pretty ?? null

  // Focus the one control, so Escape and Tab behave and a screen reader lands somewhere.
  useEffect(() => {
    if (pending !== null) closeRef.current?.focus()
  }, [pending])

  useEffect(() => {
    if (pretty === null) {
      setLines(null)
      return
    }
    let live = true
    void (async () => {
      try {
        const { grammarFor, tokenizeFence } = await import('@/editor/markdown/fenceTokens')
        const spec = await grammarFor('json')
        if (!live || spec === null) return
        setLines(tokenizeFence(spec, pretty))
      } catch {
        // Uncoloured is a complete answer; the `<pre>` below renders `pretty` either way.
      }
    })()
    return () => {
      live = false
    }
  }, [pretty])

  if (pending === null) return null
  const dismiss = () => useLogDetail.getState().dismiss()

  return (
    <OverlayCard label="Log line" onDismiss={dismiss}>
      <div
        className={styles.dialog}
        onKeyDown={(ev) => {
          if (ev.key === 'Escape') {
            ev.stopPropagation()
            dismiss()
          }
        }}
      >
        <div className={styles.head}>
          <h2 className={styles.title}>Log line</h2>
          <div className={styles.actions}>
            <button
              type="button"
              className={styles.button}
              disabled={pending.detail === null}
              onClick={() => {
                const raw = pending.detail?.raw
                if (raw === undefined) return
                // The *raw* line, not the pretty one: what goes in a ticket or through `grep`
                // is the bytes the program emitted, and reformatting somebody's evidence on
                // the way to their clipboard is not this card's business.
                // `copyText` carries the `execCommand` fallback for the WebKit contexts
                // where `navigator.clipboard` rejects, and answers false rather than doing
                // nothing quietly — which is the failure this codebase keeps having to fix.
                void copyText(raw).then((ok) =>
                  notify(ok ? 'Log line copied' : 'Could not copy the log line', {
                    kind: ok ? 'ok' : 'warn',
                  }),
                )
              }}
            >
              Copy JSON
            </button>
            <button ref={closeRef} type="button" className={styles.button} onClick={dismiss}>
              Close
            </button>
          </div>
        </div>
        <div className={styles.body}>
          {pending.gone ? (
            <p className={styles.note}>
              This line is no longer kept. cide holds the last couple of thousand log lines per
              session; older ones are dropped as new output arrives.
            </p>
          ) : pending.detail === null ? (
            <p className={styles.note}>Looking it up…</p>
          ) : (
            <pre className={styles.json}>
              {lines === null
                ? pending.detail.pretty
                : lines.map((tokens, i) => (
                    // The index is the key and the list never reorders — it is one immutable
                    // document rendered once per open.
                    <span key={i}>
                      {tokens.map((token, j) => (
                        <span key={j} className={token.cls ?? undefined}>
                          {token.text}
                        </span>
                      ))}
                      {'\n'}
                    </span>
                  ))}
            </pre>
          )}
        </div>
      </div>
    </OverlayCard>
  )
}
