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
 * Since M42 the same card is where an **agent run's tool call** opens: the opencode stream draws
 * a call as one line (`● bash  cargo test  1.2s #7`), the `#7` names the event in the same ring,
 * and a click on it lands here — with the event drawn as what it is (the input, the whole output,
 * the error) rather than as the JSON envelope around it. The model's own text opens the same way.
 * `viewOf` (`logDetailModel.ts`) decides which shape the raw line has; anything it does not
 * recognise is still the pretty JSON, so nothing a program can print ends up with no view at
 * all. A tool call and the model's text are drawn under a date and a 24-hour clock — the call's
 * own start where the harness recorded one, the line's arrival where it did not (see that
 * module's header) — because a run is mostly read after the fact and *when* is the first
 * question.
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
import { stamp, viewOf, type ToolView } from './logDetailModel'
import styles from './LogDetailCard.module.css'

interface Token {
  readonly text: string
  readonly cls: string | null
}

export function LogDetailCard() {
  const pending = useLogDetail((s) => s.pending)
  const [lines, setLines] = useState<Token[][] | null>(null)
  const closeRef = useRef<HTMLButtonElement>(null)
  const raw = pending?.detail?.raw ?? null
  const pretty = pending?.detail?.pretty ?? null
  const view = raw === null ? null : viewOf(raw)
  // `u64` on the wire is a `bigint` here — `AgentsPanel/adapt.ts`'s rule, and its reason:
  // arithmetic on it against a `number` throws inside the render.
  const recordedAt = pending?.detail ? Number(pending.detail.recordedUnixMs) : null

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

  const copy = (text: string, what: string) => {
    // `copyText` carries the `execCommand` fallback for the WebKit contexts where
    // `navigator.clipboard` rejects, and answers false rather than doing nothing quietly —
    // which is the failure this codebase keeps having to fix.
    void copyText(text).then((ok) =>
      notify(ok ? `${what} copied` : `Could not copy the ${what.toLowerCase()}`, {
        kind: ok ? 'ok' : 'warn',
      }),
    )
  }

  const heading =
    view?.kind === 'tool'
      ? view.tool
      : view?.kind === 'text'
        ? 'The model said'
        : // M62: a run's pane draws none of the reasoning any more, only `∴ thought  4.1s #8`,
          // so this card is where a thinking block is read. A different claim from "said", and
          // deliberately a different string.
          view?.kind === 'reasoning'
          ? 'The model thought'
          : 'Log line'

  return (
    <OverlayCard label={heading} onDismiss={dismiss}>
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
          <div className={styles.heading}>
            <h2 className={styles.title}>{heading}</h2>
            {view?.kind === 'tool' && view.title !== '' && (
              <span className={styles.subtitle} title={view.title}>
                {view.title}
              </span>
            )}
          </div>
          <div className={styles.actions}>
            {view?.kind === 'tool' && view.output !== '' && (
              <button
                type="button"
                className={styles.button}
                onClick={() => copy(view.output, 'Output')}
              >
                Copy output
              </button>
            )}
            <button
              type="button"
              className={styles.button}
              disabled={pending.detail === null}
              onClick={() => {
                // The *raw* line, not the pretty one: what goes in a ticket or through `grep`
                // is the bytes the program emitted, and reformatting somebody's evidence on
                // the way to their clipboard is not this card's business.
                if (raw !== null) copy(raw, 'Log line')
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
          ) : view?.kind === 'tool' ? (
            <ToolDetail view={view} recordedAt={recordedAt} />
          ) : view?.kind === 'text' || view?.kind === 'reasoning' ? (
            <div className={styles.sections}>
              <div className={styles.meta}>
                <When at={recordedAt} own={false} />
              </div>
              <pre className={styles.prose}>{view.text}</pre>
            </div>
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

/**
 * The date and 24-hour clock in the meta row. `own` says whose clock it is, and the tooltip
 * says so too, because the two differ by the call's duration: a harness that records the start
 * (opencode) gets the moment the command began, one that records nothing (codex) gets the
 * moment its completed line reached cide, and a reader comparing the two across runs should be
 * able to find that out without opening this file.
 */
function When({ at, own }: { at: number | null; own: boolean }) {
  if (at === null) return null
  return (
    <span
      className={styles.metaItem}
      title={own ? 'When the call started, by the harness’s clock' : 'When this line reached cide'}
    >
      {stamp(at)}
    </span>
  )
}

/** A tool call as sections: what was asked, what came back, what went wrong. */
function ToolDetail({ view, recordedAt }: { view: ToolView; recordedAt: number | null }) {
  const failed = view.status === 'error' || (view.exit !== null && view.exit !== 0)
  return (
    <div className={styles.sections}>
      <div className={styles.meta}>
        <span className={failed ? styles.badgeBad : styles.badgeOk}>
          {view.status === 'error'
            ? 'failed'
            : view.exit !== null && view.exit !== 0
              ? `exit ${view.exit}`
              : view.status || 'done'}
        </span>
        <When at={view.startedAt ?? recordedAt} own={view.startedAt !== null} />
        {view.duration !== null && <span className={styles.metaItem}>{view.duration}</span>}
      </div>
      <section className={styles.section}>
        <h3 className={styles.sectionTitle}>{view.command !== null ? 'Command' : 'Input'}</h3>
        <pre className={styles.code}>{view.command ?? view.input}</pre>
      </section>
      {view.error !== null && (
        <section className={styles.section}>
          <h3 className={styles.sectionTitle}>Error</h3>
          <pre className={`${styles.code} ${styles.codeBad}`}>{view.error}</pre>
        </section>
      )}
      <section className={styles.section}>
        <h3 className={styles.sectionTitle}>Output</h3>
        {view.output === '' ? (
          <p className={styles.note}>No output.</p>
        ) : (
          <pre className={styles.code}>{view.output}</pre>
        )}
      </section>
    </div>
  )
}
