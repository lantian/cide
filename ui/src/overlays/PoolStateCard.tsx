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
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { Modal } from './ModalShell'
import { Button } from '@/kit/components/Button'
import { Spinner } from '@/kit/components/Feedback'
import { Dialog } from '@/kit/components/Overlay'
import { Badge } from '@/kit/components/Status'
import { Select } from '@/kit/components/Select'
import { Section, Table, type Column } from '@/kit/components/Surface'
import { useWorkspace } from '@/store/workspace'
import { agentEvents, agentDefs, pools as poolsApi } from '@/ipc/client'
import { errorText } from '@/ipc/errorText'
import type {
  LlmModelTest,
  PoolEntry,
  PoolEntryState,
  PoolEntryStats,
  PoolEvent,
  ProjectId,
  PoolRefusal,
  PoolRunRef,
  PoolSkipped,
  PoolState,
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
    case 'variant':
      return 'refused the variant'
    case 'rejected':
      return 'rejected the request'
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

/*
 * "Recent decisions" is a table, one column per fact. (pool stats) It was one sentence per
 * event — `developer started on openrouter/x (entry 2 of 6) — passed over …` — and asked to be
 * redrawn because a column of sentences cannot be scanned: which model, which run, which
 * project were all at a different offset on every line. The four helpers below are that
 * sentence cut at its joints.
 */

const EVENT_BADGE = {
  started: { text: 'started', tone: 'green' },
  refused: { text: 'refused', tone: 'red' },
  reset: { text: 'reset', tone: 'neutral' },
} as const

function eventModel(event: PoolEvent): string {
  const kind = event.kind
  switch (kind.kind) {
    case 'started':
      return `${flag(kind.entry)} · ${kind.index + 1}/${kind.of}`
    case 'refused':
      return flag(kind.entry)
    case 'reset':
      return kind.entry === null ? 'every bench' : flag(kind.entry)
  }
}

function eventDetails(event: PoolEvent): string {
  const kind = event.kind
  switch (kind.kind) {
    case 'started': {
      const parts: string[] = []
      if (kind.benchedFallback) parts.push('although benched: nothing after it had room')
      if (kind.skipped.length > 0) parts.push(`passed over ${skippedText(kind.skipped)}`)
      return parts.join('; ')
    }
    case 'refused':
      return `${refusalText(kind.reason)} — benched until ${clock(kind.untilUnixMs)}`
    case 'reset':
      return kind.rewound === 0 ? '' : `${kind.rewound} waiting run(s) moved back`
  }
}

/*
 * An entry's figures since cide started. (pool stats) Speeds are generated tokens (output and
 * reasoning) over a request's own duration, start to finish — so they include the time to first
 * token, and a long prompt reads as a slower model. The average is Σtokens / Σtime, not a mean
 * of rates: a two-token step that took a second would otherwise drag it down as hard as a
 * two-thousand-token one.
 */

/** `1.2M`, `46k`, `812` — the stats line is read at a glance, not audited. */
function compact(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`
  if (n >= 1_000) return `${(n / 1_000).toFixed(n >= 10_000 ? 0 : 1)}k`
  return String(n)
}

function rate(tokens: number, ms: number): string {
  const perSecond = (tokens * 1000) / ms
  return perSecond < 10 ? perSecond.toFixed(1) : String(Math.round(perSecond))
}

function statsText(stats: PoolEntryStats): string | null {
  if (stats.started === 0 && stats.requests === 0) return null
  const parts = [`${stats.started} run${stats.started === 1 ? '' : 's'}`]
  parts.push(`${compact(stats.requests)} request${stats.requests === 1 ? '' : 's'}`)
  if (stats.requests > 0) {
    const cached = stats.cacheRead > 0 ? ` (+${compact(stats.cacheRead)} cached)` : ''
    parts.push(`in ${compact(stats.input)}${cached}`)
    const thought = stats.reasoning > 0 ? ` (+${compact(stats.reasoning)} reasoning)` : ''
    parts.push(`out ${compact(stats.output)}${thought}`)
  }
  if (stats.slowest !== null && stats.fastest !== null && stats.ratedMs > 0) {
    const min = rate(stats.slowest.tokens, stats.slowest.ms)
    const avg = rate(stats.ratedTokens, stats.ratedMs)
    const max = rate(stats.fastest.tokens, stats.fastest.ms)
    parts.push(`speed ${min} / ${avg} / ${max} tok/s`)
  }
  if (stats.quickestMs !== null && stats.longestMs !== null && stats.timed > 0) {
    const avg = stats.timedMs / stats.timed
    parts.push(`response ${duration(stats.quickestMs)} / ${duration(avg)} / ${duration(stats.longestMs)}`)
  }
  return parts.join(' · ')
}

/** A request's response time: `850ms`, `4.2s`, `1m 12s`. */
function duration(ms: number): string {
  if (ms < 1000) return `${Math.round(ms)}ms`
  if (ms < 60_000) return `${(ms / 1000).toFixed(ms < 10_000 ? 1 : 0)}s`
  const s = Math.round(ms / 1000)
  return `${Math.floor(s / 60)}m ${s % 60}s`
}

/**
 * The pool the card opens on: the one something happened on most recently. (pool stats)
 *
 * The newest event naming a pool that still exists — a start, a refusal — is the pool a person
 * opening this card is most likely asking about. With no such event (nothing has run since cide
 * started, or the pool was renamed since), the pool with the most runs on it now, and failing
 * that the first. Events arrive newest first.
 */
function activePool(report: PoolStateReport): string | null {
  const names = new Set(report.pools.map((p) => p.name))
  const recent = report.events.find((e) => e.pool !== null && names.has(e.pool))
  if (recent?.pool != null) return recent.pool
  let busiest: PoolState | null = null
  let most = 0
  for (const pool of report.pools) {
    const running = pool.entries.reduce((n, e) => n + e.running, 0)
    if (running > most) {
      busiest = pool
      most = running
    }
  }
  return busiest?.name ?? report.pools[0]?.name ?? null
}

/** The selector row's right-hand note: how busy the pool is now, and whether any of it is benched. */
function poolDetail(pool: PoolState): string {
  const running = pool.entries.reduce((n, e) => n + e.running, 0)
  const benched = pool.entries.filter((e) => e.bench !== null).length
  const parts = [`${running} running`]
  if (benched > 0) parts.push(`${benched} benched`)
  return parts.join(', ')
}

/**
 * Whether an off-pool run's model (`provider/model`, or opencode 2's `provider/model#variant`)
 * is this entry. A model with no variant matches only the entry with none, as the backend's
 * `stats_target` reads it.
 */
function onTarget(model: string | null, entry: PoolEntry): boolean {
  if (model === null) return false
  const [target, variant = ''] = model.split('#', 2)
  return target === flag(entry) && variant === entry.variant
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
  // The stored map itself, names derived below — a selector that built the name list would be a
  // fresh array per read (`check:selectors`).
  const projects = useWorkspace((s) => s.boot?.workspace.projects)
  // One pool at a time, picked at the top. (pool stats) Every pool stacked, each with its entry
  // rows and now a stats line, was a scroll through pools nobody asked about to reach the one
  // that is working. `null` until the first report, which picks for itself: see `activePool`.
  const [chosen, setChosen] = useState<string | null>(null)
  const pools = typeof report === 'object' && report !== null ? report.pools : null
  const selected = pools?.find((p) => p.name === chosen) ?? null
  useEffect(() => {
    // Picked once, when the first report arrives or the chosen pool is renamed or removed — not
    // on every poll, or the card would jump away from the pool a person chose to read.
    if (typeof report !== 'object' || report === null) return
    if (report.pools.some((p) => p.name === chosen)) return
    setChosen(activePool(report))
  }, [report, chosen])
  // The log is filtered to the chosen pool too, keeping the events that name no pool (a reset of
  // every bench) because they are about this one as well.
  const events = useMemo(() => {
    if (typeof report !== 'object' || report === null) return null
    return report.events.filter((e) => e.pool === null || e.pool === chosen)
  }, [report, chosen])
  const columns = useMemo(() => {
    // Pools are global, so one log can hold several projects' runs. The column only when it
    // does: on a one-project log it would be the same name on every row.
    const named = new Set((events ?? []).map((e) => e.project).filter((p): p is ProjectId => p !== null))
    const projectName = (id: ProjectId | null) =>
      id === null ? '' : (projects?.[id]?.name ?? 'closed project')
    const all: Array<Column<PoolEvent> | null> = [
      { key: 'time', label: 'Time', render: (e) => <span className={styles.mono}>{clock(e.atUnixMs)}</span> },
      { key: 'pool', label: 'Pool', render: (e) => <span className={styles.nowrap}>{e.pool ?? '—'}</span> },
      {
        key: 'event',
        label: 'Event',
        render: (e) => (
          <Badge tone={EVENT_BADGE[e.kind.kind].tone} soft>
            {EVENT_BADGE[e.kind.kind].text}
          </Badge>
        ),
      },
      { key: 'model', label: 'Model', render: (e) => <span className={styles.mono}>{eventModel(e)}</span> },
      { key: 'run', label: 'Run', render: (e) => <span className={styles.who}>{e.agentLabel ?? '—'}</span> },
      named.size > 1
        ? {
            key: 'project',
            label: 'Project',
            render: (e) => <span className={styles.nowrap}>{projectName(e.project)}</span>,
          }
        : null,
      { key: 'details', label: 'Details', render: (e) => <span className={styles.details}>{eventDetails(e)}</span> },
    ]
    return all.filter((c): c is Column<PoolEvent> => c !== null)
  }, [events, projects])

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

  // And a re-read every two seconds while the card is open, for the entries' stats. (pool stats)
  // A finished step moves them but emits nothing — `AgentRegistry::note_usage` says why: a
  // broadcast per model call would redraw every window for a number only this card shows. So the
  // card that shows it asks, and only while it is open: one lock and a small report per poll.
  // Skipped while the window is hidden, where nobody is reading it.
  useEffect(() => {
    const id = window.setInterval(() => {
      if (document.visibilityState === 'visible') load()
    }, 2000)
    return () => window.clearInterval(id)
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
    // An Escape the pool selector already answered (closing its list) marks itself handled but
    // still bubbles here; dismissing the card on it would throw away the card with the list.
    if (ev.key === 'Escape' && !ev.defaultPrevented) {
      ev.stopPropagation()
      onDismiss()
    }
  }

  const anyBench =
    typeof report === 'object' && report !== null && report.pools.some((p) => p.entries.some((e) => e.bench !== null))

  return (
    <Modal onDismiss={onDismiss}>
      <Dialog
        title="Model pools"
        lead="A run starts on the first entry that has room and is not benched. A provider that refuses a run is benched for everyone until it expires or you reset it. Reset moves waiting runs back up; a run already on a model keeps it until its next dispatch."
        // Wide since "Recent decisions" became a table of up to seven columns. (pool stats)
        width="wide"
        onKeyDown={onKeyDown}
        data-audit="poolState"
        footNote={problem !== null && <span className={styles.problem}>{problem}</span>}
        actions={
          <>
            <Button disabled={!anyBench} onClick={() => reset(null)} data-audit="poolStateResetAll">
              Reset all
            </Button>
            <Button ref={close} variant="primary" onClick={onDismiss}>
              Close
            </Button>
          </>
        }
      >
        <div className={styles.body}>
          {report === null && <Spinner label="Reading…" />}
          {report === 'unsupported' && <p className={styles.dim}>This build cannot report pool state.</p>}
          {typeof report === 'object' && report !== null && (
            <>
              {report.pools.length === 0 && (
                <p className={styles.dim}>No pools are configured. Add one on the Models screen in Settings.</p>
              )}
              {report.pools.length > 0 && (
                <div className={styles.picker}>
                  <Select
                    aria-label="Pool"
                    value={chosen}
                    onChange={setChosen}
                    options={report.pools.map((pool) => ({ value: pool.name, label: pool.name, detail: poolDetail(pool) }))}
                  />
                </div>
              )}
              {selected !== null && (
                <div key={selected.name} data-audit="poolStatePool">
                  {/* The selector above already names the pool; the caption says what is under it. */}
                  <Section caption="Entries" aside={selected.description !== '' ? selected.description : undefined}>
                    <ol className={styles.entries}>
                      {selected.entries.map((entry, i) => (
                        <EntryRow
                          key={`${i}:${targetKey(entry.entry)}`}
                          pool={selected.name}
                          index={i}
                          state={entry}
                          now={now}
                          test={tests[targetKey(entry.entry)]}
                          offPool={report.offPool.filter((run) => onTarget(run.model, entry.entry))}
                          onReset={() => reset(entry.entry)}
                          onTest={() => test(entry.entry)}
                        />
                      ))}
                    </ol>
                  </Section>
                </div>
              )}
              {report.offPool.length > 0 && (
                <div data-audit="poolStateOffPool">
                  <Section caption={`Off any pool (${report.offPool.length})`}>
                    <p className={styles.dim}>
                      These runs' roles are not pointed at a pool, so no pool above applies to them: each runs on
                      its role's own model or, with none, opencode's default model. Point a role or a whole project
                      at a pool under Settings → Agents.
                    </p>
                    <ul className={styles.list}>
                      {report.offPool.map((run) => (
                        <li key={run.run} className={styles.line}>
                          <span className={styles.who}>{run.agentLabel}</span>
                          <span className={styles.dim}> — {run.model ?? 'the CLI default model'}</span>
                        </li>
                      ))}
                    </ul>
                  </Section>
                </div>
              )}
              {report.waiting.length > 0 && (
                <Section caption="Waiting">
                  <ul className={styles.list}>
                    {report.waiting.map((run) => (
                      <li key={run.run} className={styles.line}>
                        <span className={styles.who}>{runText(run)}</span>
                        {run.note !== null && <span className={styles.dim}> — {run.note}</span>}
                      </li>
                    ))}
                  </ul>
                </Section>
              )}
              <Section caption="Recent decisions">
                {events === null || events.length === 0 ? (
                  <p className={styles.dim}>Nothing yet since cide started.</p>
                ) : (
                  <div className={styles.events} data-audit="poolStateEvents">
                    <Table
                      columns={columns}
                      // Newest first, as the backend sends them.
                      rows={events}
                      rowKey={(event) => `${event.atUnixMs}:${events.indexOf(event)}`}
                    />
                  </div>
                )}
              </Section>
            </>
          )}
        </div>
      </Dialog>
    </Modal>
  )
}

/** A status tone, as the kit's badge tone: ready is done-green, full is waiting-amber. */
const BADGE_TONE = { ok: 'green', warn: 'yellow', bad: 'red' } as const

function EntryRow({
  pool,
  index,
  state,
  now,
  test,
  offPool,
  onReset,
  onTest,
}: {
  pool: string
  index: number
  state: PoolEntryState
  now: number
  test: LlmModelTest | 'running' | undefined
  /** Runs with no pool whose model is this entry's — working here, outside its limit. */
  offPool: PoolRunRef[]
  onReset: () => void
  onTest: () => void
}) {
  const { entry, bench } = state
  const stats = statsText(state.stats)
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
  const pooled = max === null ? `${state.running} running, no limit` : `${state.running} of ${max} running`
  // An off-pool run on this model is not in the count against `maxRunning` (admission never
  // placed it and cannot hold it back), but it is on this server. Named beside the count, or an
  // entry with a run working on it read as idle. (pool stats)
  const load = offPool.length > 0 ? `${pooled} +${offPool.length} off pool` : pooled
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
        <span className={styles.status} title={status.text}>
          <Badge tone={BADGE_TONE[status.tone]} soft>
            {status.text}
          </Badge>
        </span>
        <span className={styles.actions}>
          {benched && (
            <Button size="sm" onClick={onReset} data-audit="poolStateReset">
              Reset
            </Button>
          )}
          <Button
            size="sm"
            onClick={onTest}
            busy={test === 'running'}
            title="One real turn against this model. It spends quota."
          >
            {test === 'running' ? 'Testing…' : 'Test'}
          </Button>
        </span>
      </div>
      {(state.runs.length > 0 || offPool.length > 0) && (
        <div className={styles.runs}>
          on it:{' '}
          {[
            ...state.runs
              // The run's pool position only when it came from *another* pool: on this pool's own
              // row, `default 1 of 6` beside entry 1 of default read as a busy count.
              .map((run) =>
                run.position === '' || run.position.startsWith(`${pool} `) ? run.agentLabel : runText(run),
              ),
            ...offPool.map((run) => `${run.agentLabel} (off pool)`),
          ].join(', ')}
        </div>
      )}
      {stats !== null && (
        <div
          className={styles.stats}
          data-audit="poolStateStats"
          title="Since cide started, every pool naming this model. Speed: min / avg / max generated tokens per second of a request, start to finish. Then min / avg / max response time of a request, start to finish."
        >
          {stats}
        </div>
      )}
      {test !== undefined && test !== 'running' && (
        <div className={test.ok ? styles.testOk : styles.testBad}>{test.detail}</div>
      )}
    </li>
  )
}
