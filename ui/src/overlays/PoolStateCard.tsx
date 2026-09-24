/**
 * The model-pool state card. (M90)
 *
 * Asked for as *"my default pool is always starting from the last element and I have no visual
 * info why"*. Before this a pooled run's row said `default 6 of 6` and nothing about the five
 * entries before it — whether they were at their `maxRunning`, whether a provider had refused, or
 * whether the run had simply been walked down its list by a failover an hour ago. All three are
 * decided in `crates/cide-app/src/agents.rs`'s admission and failover under one lock, and this
 * card is that registry's one read of them (`llm_pool_state`), redrawn on every
 * `cide://agents-changed` because every event that can move it already emits one.
 *
 * Per entry: load against its limit, whether its provider is usable, which runs are on it, and
 * the **bench** — a refusal steers every later admission around its target until it expires or a
 * person presses **Reset** here, which is the button for "I have started the local server now".
 * **Test** beside it is the Models screen's probe, one real turn, and says so: it spends quota.
 *
 * `AboutCard`'s two rules for the ground: focus lands on a button before the first paint, and
 * Escape is answered on a wrapper *inside* the card rather than on `document`.
 */
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { OverlayCard } from './ModalShell'
import { agentEvents, agentDefs, pools as poolsApi } from '@/ipc/client'
import { errorText } from '@/ipc/errorText'
import type {
  LlmModelTest,
  PoolEntry,
  PoolEntryState,
  PoolEvent,
  PoolRefusal,
  PoolRunRef,
  PoolSkipped,
  PoolStateReport,
} from '@/ipc/generated'
import styles from './PoolStateCard.module.css'

/** `provider/model`, the way every row and every argv in cide spells a candidate. */
function flag(entry: PoolEntry): string {
  return `${entry.provider}/${entry.model}`
}

/** A target's key: the three fields `PoolEntry::same_target` compares. */
function targetKey(entry: PoolEntry): string {
  return `${entry.provider}\u0000${entry.model}\u0000${entry.variant}`
}

function refusalText(reason: PoolRefusal): string {
  switch (reason) {
    case 'rateLimited':
      return 'rate limited'
    case 'unreachable':
      return 'unreachable'
    case 'auth':
      return 'refused the credential'
  }
}

/** `2m 10s` left, from the backend's clock plus how long ago it was read. */
function remaining(until: number, now: number): string {
  const s = Math.max(0, Math.ceil((until - now) / 1000))
  const m = Math.floor(s / 60)
  return m > 0 ? `${m}m ${s % 60}s` : `${s}s`
}

function clock(at: number): string {
  return new Date(at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })
}

function skippedText(skipped: PoolSkipped[]): string {
  return skipped
    .map((s) =>
      s.skip.kind === 'full'
        ? `${flag(s.entry)} full ${s.skip.running}/${s.skip.max}`
        : `${flag(s.entry)} benched (${refusalText(s.skip.reason)})`,
    )
    .join(', ')
}

function eventText(event: PoolEvent): string {
  const who = event.agentLabel ?? 'a run'
  const kind = event.kind
  switch (kind.kind) {
    case 'started': {
      const where = `${flag(kind.entry)} (entry ${kind.index + 1} of ${kind.of})`
      const fallback = kind.benchedFallback ? ', although benched: nothing after it had room' : ''
      const passed = kind.skipped.length > 0 ? ` — passed over ${skippedText(kind.skipped)}` : ''
      return `${who} started on ${where}${fallback}${passed}`
    }
    case 'refused':
      return `${flag(kind.entry)} ${refusalText(kind.reason)} for ${who} — benched`
    case 'reset': {
      const what = kind.entry === null ? 'every bench' : flag(kind.entry)
      const moved = kind.rewound === 0 ? '' : `; ${kind.rewound} waiting run(s) moved back`
      return `reset ${what}${moved}`
    }
  }
}

function runText(run: PoolRunRef): string {
  return run.position === '' ? run.agentLabel : `${run.agentLabel} (${run.position})`
}

export function PoolStateCard({ onDismiss }: { onDismiss: () => void }) {
  const [report, setReport] = useState<PoolStateReport | null | 'unsupported'>(null)
  // The webview's clock at the read, beside the backend's: a countdown is the backend's `now`
  // plus however long ago that was, so a webview clock a few seconds off cannot bench an entry
  // that has expired or free one that has not.
  const [readAt, setReadAt] = useState(0)
  const [tick, setTick] = useState(0)
  const [problem, setProblem] = useState<string | null>(null)
  const [tests, setTests] = useState<Record<string, LlmModelTest | 'running'>>({})
  const close = useRef<HTMLButtonElement>(null)

  const load = useCallback(() => {
    void poolsApi.state().then((next) => {
      setReport(next ?? 'unsupported')
      setReadAt(Date.now())
    })
  }, [])

  useLayoutEffect(() => {
    close.current?.focus()
  }, [])

  useEffect(() => {
    load()
    const off = agentEvents.onChanged(() => load())
    return () => {
      void off.then((stop) => stop())
    }
  }, [load])

  // One tick a second for the countdowns, and a re-read when one of them runs out — an expired
  // bench changes nothing in the registry, so nothing would emit to say so.
  useEffect(() => {
    const id = window.setInterval(() => setTick((n) => n + 1), 1000)
    return () => window.clearInterval(id)
  }, [])

  const now = typeof report === 'object' && report !== null ? report.nowUnixMs + (Date.now() - readAt) : 0
  useEffect(() => {
    if (typeof report !== 'object' || report === null) return
    const expired = report.pools.some((pool) =>
      pool.entries.some((e) => e.bench !== null && e.bench.untilUnixMs <= now),
    )
    if (expired) load()
    // `tick` is the dependency that matters; `now` is derived from it.
  }, [tick]) // eslint-disable-line react-hooks/exhaustive-deps

  const reset = (entry: PoolEntry | null) => {
    setProblem(null)
    poolsApi.reset(entry).then(load, (error: unknown) => setProblem(errorText(error)))
  }

  const test = (entry: PoolEntry) => {
    const key = targetKey(entry)
    setTests((t) => ({ ...t, [key]: 'running' }))
    void agentDefs.testModel(null, flag(entry)).then((verdict) => {
      setTests((t) => ({
        ...t,
        [key]: verdict ?? { model: flag(entry), ok: false, detail: 'This build cannot run a test.' },
      }))
    })
  }

  const onKeyDown = (ev: React.KeyboardEvent) => {
    if (ev.key === 'Escape') {
      ev.stopPropagation()
      onDismiss()
    }
  }

  const anyBench =
    typeof report === 'object' && report !== null && report.pools.some((p) => p.entries.some((e) => e.bench !== null))

  return (
    <OverlayCard label="Model pools" onDismiss={onDismiss}>
      <div className={styles.dialog} onKeyDown={onKeyDown} data-audit="poolState">
        <div className={styles.head}>
          <h2 className={styles.title}>Model pools</h2>
          <p className={styles.lede}>
            A run starts on the first entry that has room and is not benched. A provider that refuses a run is
            benched for everyone until it expires or you reset it. Reset moves waiting runs back up; a run already
            on a model keeps it until its next dispatch.
          </p>
        </div>
        <div className={styles.body}>
          {report === null && <p className={styles.dim}>Reading…</p>}
          {report === 'unsupported' && <p className={styles.dim}>This build cannot report pool state.</p>}
          {typeof report === 'object' && report !== null && (
            <>
              {report.pools.length === 0 && (
                <p className={styles.dim}>No pools are configured. Add one on the Models screen in Settings.</p>
              )}
              {report.pools.map((pool) => (
                <section key={pool.name} className={styles.pool} data-audit="poolStatePool">
                  <h3 className={styles.poolName}>
                    {pool.name}
                    {pool.description !== '' && <span className={styles.dim}> — {pool.description}</span>}
                  </h3>
                  <ol className={styles.entries}>
                    {pool.entries.map((entry, i) => (
                      <EntryRow
                        key={`${i}:${targetKey(entry.entry)}`}
                        pool={pool.name}
                        index={i}
                        state={entry}
                        now={now}
                        test={tests[targetKey(entry.entry)]}
                        onReset={() => reset(entry.entry)}
                        onTest={() => test(entry.entry)}
                      />
                    ))}
                  </ol>
                </section>
              ))}
              {report.offPool.length > 0 && (
                <section className={styles.pool} data-audit="poolStateOffPool">
                  <h3 className={styles.poolName}>Off any pool ({report.offPool.length})</h3>
                  <p className={styles.dim}>
                    These runs' roles are not pointed at a pool, so no pool above applies to them: each runs on its
                    role's own model or, with none, opencode's default model. Point a role or a whole project at a
                    pool under Settings → Agents.
                  </p>
                  <ul className={styles.list}>
                    {report.offPool.map((run) => (
                      <li key={run.run} className={styles.line}>
                        <span className={styles.who}>{run.agentLabel}</span>
                        <span className={styles.dim}> — {run.model ?? 'the CLI default model'}</span>
                      </li>
                    ))}
                  </ul>
                </section>
              )}
              {report.waiting.length > 0 && (
                <section className={styles.pool}>
                  <h3 className={styles.poolName}>Waiting</h3>
                  <ul className={styles.list}>
                    {report.waiting.map((run) => (
                      <li key={run.run} className={styles.line}>
                        <span className={styles.who}>{runText(run)}</span>
                        {run.note !== null && <span className={styles.dim}> — {run.note}</span>}
                      </li>
                    ))}
                  </ul>
                </section>
              )}
              <section className={styles.pool}>
                <h3 className={styles.poolName}>Recent decisions</h3>
                {report.events.length === 0 ? (
                  <p className={styles.dim}>Nothing yet since cide started.</p>
                ) : (
                  <ul className={styles.list} data-audit="poolStateEvents">
                    {report.events.map((event, i) => (
                      <li key={`${event.atUnixMs}:${i}`} className={styles.line}>
                        <span className={styles.time}>{clock(event.atUnixMs)}</span>
                        {event.pool !== null && <span className={styles.dim}>{event.pool} </span>}
                        {eventText(event)}
                      </li>
                    ))}
                  </ul>
                )}
              </section>
            </>
          )}
        </div>
        <div className={styles.footer}>
          {problem !== null && <span className={styles.problem}>{problem}</span>}
          <button
            type="button"
            className={styles.secondary}
            disabled={!anyBench}
            onClick={() => reset(null)}
            data-audit="poolStateResetAll"
          >
            Reset all
          </button>
          <button ref={close} type="button" className={styles.button} onClick={onDismiss}>
            Close
          </button>
        </div>
      </div>
    </OverlayCard>
  )
}

function EntryRow({
  pool,
  index,
  state,
  now,
  test,
  onReset,
  onTest,
}: {
  pool: string
  index: number
  state: PoolEntryState
  now: number
  test: LlmModelTest | 'running' | undefined
  onReset: () => void
  onTest: () => void
}) {
  const { entry, bench } = state
  const benched = bench !== null && bench.untilUnixMs > now
  const max = entry.maxRunning ?? null
  const full = max !== null && state.running >= max
  let status: { text: string; tone: 'ok' | 'warn' | 'bad' }
  if (state.provider === 'disabled') status = { text: 'provider switched off — never tried', tone: 'bad' }
  else if (benched)
    status = {
      text: `benched: ${refusalText(bench.reason)} for ${bench.agentLabel}, ${remaining(bench.untilUnixMs, now)} left`,
      tone: 'bad',
    }
  else if (full) status = { text: 'full', tone: 'warn' }
  else if (state.provider === 'missing') status = { text: 'provider not configured in cide', tone: 'warn' }
  else status = { text: 'ready', tone: 'ok' }

  // Spelled out: a bare `0/3` beside `ready` read as "0 of 3 ready" rather than as the load
  // against `maxRunning` it is.
  const load = max === null ? `${state.running} running, no limit` : `${state.running} of ${max} running`
  return (
    <li className={styles.entry} data-audit="poolStateEntry">
      <div className={styles.entryHead}>
        <span className={styles.index}>{index + 1}.</span>
        <span className={styles.model}>
          {flag(entry)}
          {entry.variant !== '' && <span className={styles.dim}> [{entry.variant}]</span>}
        </span>
        <span className={styles.load} title="Runs on this model now, in every project, against its maxRunning">
          {load}
        </span>
        <span className={`${styles.status} ${styles[status.tone]}`}>{status.text}</span>
        <span className={styles.actions}>
          {benched && (
            <button type="button" className={styles.secondary} onClick={onReset} data-audit="poolStateReset">
              Reset
            </button>
          )}
          <button
            type="button"
            className={styles.secondary}
            onClick={onTest}
            disabled={test === 'running'}
            title="One real turn against this model. It spends quota."
          >
            {test === 'running' ? 'Testing…' : 'Test'}
          </button>
        </span>
      </div>
      {state.runs.length > 0 && (
        <div className={styles.runs}>
          on it:{' '}
          {state.runs
            // The run's pool position only when it came from *another* pool: on this pool's own
            // row, `default 1 of 6` beside entry 1 of default read as a busy count.
            .map((run) => (run.position === '' || run.position.startsWith(`${pool} `) ? run.agentLabel : runText(run)))
            .join(', ')}
        </div>
      )}
      {test !== undefined && test !== 'running' && (
        <div className={test.ok ? styles.testOk : styles.testBad}>{test.detail}</div>
      )}
    </li>
  )
}
