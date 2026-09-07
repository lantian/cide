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
 * `viewOf` decides which shape the raw line has; anything it does not recognise is still the
 * pretty JSON, so nothing a program can print ends up with no view at all.
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

/** A tool call, as the opencode stream reports one once it has completed. */
interface ToolView {
  readonly kind: 'tool'
  readonly tool: string
  readonly title: string
  /** `completed`, `error`, or whatever the CLI said. */
  readonly status: string
  readonly duration: string | null
  /** A shell command's exit code, when the CLI recorded one. */
  readonly exit: number | null
  /** The one string input a call usually has — a command, a path — or `null` for a document. */
  readonly command: string | null
  /** The whole input as pretty JSON, for the document case and the card's copy action. */
  readonly input: string
  readonly output: string
  readonly error: string | null
}

/** The model's own words. */
interface TextView {
  readonly kind: 'text'
  readonly text: string
}

type View = ToolView | TextView

/**
 * `cide_cide_task_get` reads as `cide_task_get` — the renderer's `display_tool`, restated here
 * because the raw event still carries the doubled server prefix.
 */
function displayTool(tool: string): string {
  return tool.startsWith('cide_cide_') ? `cide_${tool.slice('cide_cide_'.length)}` : tool
}

/** `8ms`, `1.2s`, `2m05s` — the renderer's three shapes, so the card agrees with the line. */
function duration(ms: number): string {
  if (ms < 1_000) return `${ms}ms`
  if (ms < 60_000) return `${(ms / 1_000).toFixed(1)}s`
  const m = Math.floor(ms / 60_000)
  const s = Math.floor((ms % 60_000) / 1_000)
  return `${m}m${String(s).padStart(2, '0')}s`
}

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null
}

function asString(value: unknown): string | null {
  return typeof value === 'string' ? value : null
}

/**
 * What the raw line is, when it is one of the shapes the card draws specially.
 *
 * Read off the same fields `cide_agents::harness::opencode::render_tool` reads, and nothing
 * else: `part.tool`, `part.state.{status,title,input,output,error,time,metadata}`. A line that
 * is not JSON, or JSON of another shape, answers `null` and stays the JSON view.
 */
function viewOf(raw: string): View | null {
  let event: unknown
  try {
    event = JSON.parse(raw)
  } catch {
    return null
  }
  const record = asRecord(event)
  if (record === null) return null
  // A codex event carries its call under `item`, not `part` (M44).
  if (record.type === 'item.completed') return codexItemView(asRecord(record.item))
  const part = asRecord(record.part)
  if (part === null) return null
  switch (record.type) {
    case 'text': {
      const text = asString(part.text)
      return text === null ? null : { kind: 'text', text }
    }
    case 'tool_use': {
      const state = asRecord(part.state) ?? {}
      const input = asRecord(state.input) ?? {}
      const strings = Object.values(input).filter((v): v is string => typeof v === 'string')
      // One string input is the call's whole argument (a command, a path); a document is a
      // document. `Object.keys` rather than `strings` for the test, so `{ "command": "ls",
      // "timeout": 5 }` still reads as a command.
      const command = Object.keys(input).length === 1 && strings.length === 1 ? strings[0]! : null
      const time = asRecord(state.time)
      const start = typeof time?.start === 'number' ? time.start : null
      const end = typeof time?.end === 'number' ? time.end : null
      const metadata = asRecord(state.metadata)
      const exit = typeof metadata?.exit === 'number' ? metadata.exit : null
      return {
        kind: 'tool',
        tool: displayTool(asString(part.tool) ?? 'tool'),
        title: (asString(state.title) ?? '').trim() || strings.join(' '),
        status: asString(state.status) ?? '',
        duration: start !== null && end !== null ? duration(Math.max(0, end - start)) : null,
        exit,
        command,
        input: JSON.stringify(input, null, 2),
        output: asString(state.output) ?? '',
        error: asString(state.error),
      }
    }
    default:
      return null
  }
}

/**
 * A codex `item.completed` as the card draws it (M44). Codex's items carry the whole call —
 * the command and its aggregated output, an MCP call's arguments and result, a file change's
 * list — under names of their own, so this is the opencode `tool_use` reading restated over that
 * shape. `cide_agents::harness::codex::render_item` is the one-line twin. An item the card has
 * no view for falls to the pretty JSON, which is still the whole event.
 */
function codexItemView(item: Record<string, unknown> | null): View | null {
  if (item === null) return null
  const status = asString(item.status) ?? ''
  switch (item.type) {
    case 'agent_message': {
      const text = asString(item.text)
      return text === null ? null : { kind: 'text', text }
    }
    case 'command_execution': {
      const command = asString(item.command)
      const output = asString(item.aggregated_output) ?? ''
      const trimmed = output
        .split('\n')
        .map((line) => line.trim())
        .filter((line) => line !== '')
      const last = trimmed.length === 0 ? null : trimmed[trimmed.length - 1]!
      return {
        kind: 'tool',
        tool: 'shell',
        title: command ?? '',
        status,
        duration: null,
        exit: typeof item.exit_code === 'number' ? item.exit_code : null,
        command,
        input: JSON.stringify({ command }, null, 2),
        output,
        error:
          status === 'failed'
            ? (last ?? 'failed')
            : status === 'declined'
              ? 'declined by the sandbox or the approval policy'
              : null,
      }
    }
    case 'mcp_tool_call': {
      const server = asString(item.server) ?? ''
      const tool = asString(item.tool) ?? 'tool'
      const args = asRecord(item.arguments) ?? {}
      const strings = Object.values(args).filter((v): v is string => typeof v === 'string')
      const command = Object.keys(args).length === 1 && strings.length === 1 ? strings[0]! : null
      const error = asRecord(item.error)
      return {
        kind: 'tool',
        // cide's own server by the tool's name, as the renderer spells it; another server's
        // with the server in front.
        tool: server === '' || server === 'cide' ? tool : `${server}:${tool}`,
        title: strings.join(' '),
        status,
        duration: null,
        exit: null,
        command,
        input: JSON.stringify(args, null, 2),
        output: item.result === undefined ? '' : JSON.stringify(item.result, null, 2),
        error: asString(error?.message) ?? (status === 'failed' ? 'failed' : null),
      }
    }
    case 'file_change': {
      const changes = Array.isArray(item.changes) ? item.changes : []
      const lines = changes.map((change) => {
        const record = asRecord(change)
        const kind = asString(record?.kind) ?? 'update'
        const glyph = kind === 'add' ? '+' : kind === 'delete' ? '-' : '~'
        return `${glyph}${asString(record?.path) ?? ''}`
      })
      return {
        kind: 'tool',
        tool: 'edit',
        title: lines.join(' '),
        status,
        duration: null,
        exit: null,
        command: null,
        input: JSON.stringify(changes, null, 2),
        output: '',
        error: status === 'failed' ? 'failed' : null,
      }
    }
    default:
      return null
  }
}

export function LogDetailCard() {
  const pending = useLogDetail((s) => s.pending)
  const [lines, setLines] = useState<Token[][] | null>(null)
  const closeRef = useRef<HTMLButtonElement>(null)
  const raw = pending?.detail?.raw ?? null
  const pretty = pending?.detail?.pretty ?? null
  const view = raw === null ? null : viewOf(raw)

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
    view?.kind === 'tool' ? view.tool : view?.kind === 'text' ? 'The model said' : 'Log line'

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
            <ToolDetail view={view} />
          ) : view?.kind === 'text' ? (
            <pre className={styles.prose}>{view.text}</pre>
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

/** A tool call as sections: what was asked, what came back, what went wrong. */
function ToolDetail({ view }: { view: ToolView }) {
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
