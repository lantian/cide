/**
 * The M0 GO/NO-GO measurement.
 *
 * The PTY transport assumes raw bytes can cross the IPC boundary fast enough to carry a
 * terminal under load. If they cannot, the design changes to Rust-side VT parsing with
 * dirty-row diffs — and that has to be decided before anything is built on top of the
 * current assumption, which is why this runs before the rest of the app exists.
 *
 * # Detecting the degraded path
 *
 * Tauri's fast path fails silently. If WebKitGTK is older than 2.40 or a CSP rule blocks
 * the `ipc:` scheme, `ipc-protocol.js` catches the fetch rejection, sets an internal
 * `customProtocolIpcFailed` flag *permanently* and falls back to string `postMessage`.
 * That flag lives in the injected script's closure and is not reachable from here.
 *
 * But the *shape* of the response gives it away directly: over the custom protocol a
 * `Response::new(Vec<u8>)` arrives as an `ArrayBuffer`, and over `postMessage` the same
 * value is JSON — a plain array of numbers. So we ask for a few bytes and look at what
 * comes back. That is a fact about the transport rather than a guess from a timing
 * threshold.
 */
import { diag, type IpcHealth } from '@/ipc/client'

export interface BenchRow {
  label: string
  bytes: number
  iterations: number
  mibPerSec: number
  meanMs: number
  p99Ms: number
}

export interface BenchReport {
  health: IpcHealth
  rows: BenchRow[]
}

const SIZES = [1024, 8 * 1024, 64 * 1024, 256 * 1024]

/**
 * The GO/NO-GO floor, in MiB/s on the raw pull path. See `BENCH.md`.
 *
 * Exported so the verdict line, the boot probe and anything else that wants to say
 * "degraded" all read the same number instead of each carrying its own 30.
 */
export const FLOOR_MIB_PER_SEC = 30

/**
 * Pull iterations per payload size.
 *
 * This was 200, which made the committed p99 close to meaningless: at 200 samples the
 * 99th percentile is the third-worst measurement, so a single scheduler preemption anywhere
 * in the run *is* the p99. At 10 000 it is the hundredth-worst, which is a tail rather than
 * an anecdote — and the tail is the number that matters here, because a PTY under load
 * stalls on the worst frames, not the average ones.
 *
 * The cost is bounded and paid only by someone running the gate: ~3.1 GiB moved in total
 * across the four sizes, which at the throughputs in BENCH.md is well under a minute.
 */
export const PULL_ITERATIONS = 10_000

/**
 * The boot probe's payload, and why it is 8 KiB.
 *
 * `cide-pty`'s `FLUSH_BYTES` is 8 KiB, so this is the size a real PTY frame actually is —
 * probing at 64 bytes would measure the one size Tauri routes through `webview.eval` even
 * on a healthy transport, and probing at 256 KiB would make a degraded boot pay for a
 * quarter-megabyte JSON array before it can report that it is degraded.
 */
const PROBE_BYTES = 8 * 1024

/**
 * Round trips the boot probe makes, and why more than one.
 *
 * The question is "is this transport *capable* of PTY throughput", so the best sample is the
 * honest answer and a single one leaves it to chance whether the first IPC of the process
 * lands next to a compositor frame. Three 8 KiB round trips is 24 KiB and a fraction of a
 * millisecond on the fast path; on the degraded path it is three small evals, which is
 * exactly the case worth spending them on.
 */
const PROBE_ITERATIONS = 3

function stats(durations: number[], bytesEach: number) {
  const sorted = [...durations].sort((a, b) => a - b)
  const total = durations.reduce((a, b) => a + b, 0)
  const mean = total / durations.length
  const p99 = sorted[Math.min(sorted.length - 1, Math.floor(sorted.length * 0.99))] ?? mean
  const mib = (bytesEach * durations.length) / (1024 * 1024)
  return { mibPerSec: mib / (total / 1000), meanMs: mean, p99Ms: p99 }
}

/** True when raw payloads arrive as bytes rather than as a JSON number array. */
export async function detectCustomProtocol(): Promise<boolean> {
  const probe = await diag.echoBytes(64)
  return isRawBytes(probe)
}

/** The shape test of the module docs, as one predicate both callers share. */
function isRawBytes(value: unknown): boolean {
  return value instanceof ArrayBuffer || ArrayBuffer.isView(value)
}

/** How many bytes came back, whichever shape the transport delivered them in. */
function payloadLength(value: unknown): number {
  if (value instanceof ArrayBuffer) return value.byteLength
  if (ArrayBuffer.isView(value)) return value.byteLength
  // The degraded path: a JSON array of numbers, which has `length` and no `byteLength`.
  return Array.isArray(value) ? value.length : 0
}

/**
 * The boot probe: which IPC path did this window actually get, and is it fast enough?
 *
 * # Why this is not the benchmark
 *
 * [`runBench`] answers a design question — *can this architecture work at all* — and it is
 * expensive enough to be worth a dedicated launch. This answers an operational one for every
 * ordinary boot: **did WebKitGTK silently fall back to `postMessage` on this machine.** That
 * failure has no error, no crash and no Rust-side signal; the only symptom is a terminal that
 * feels inexplicably sluggish under load, because every PTY frame is now a JSON array of
 * decimal numbers spelled into a `webview.eval`. A user who hits it should learn it from a
 * log line, not from a hunch.
 *
 * Cost is {@link PROBE_ITERATIONS} round trips of {@link PROBE_BYTES} — 24 KiB and a fraction
 * of a millisecond — so it is affordable unconditionally, which is the point: guarded behind
 * `benchMode()` it ran only during a benchmark, i.e. never on the boots that could actually
 * be degraded.
 *
 * Reports to `diag_report_ipc`, which logs it and warns loudly on the degraded path.
 */
export async function probeIpc(): Promise<IpcHealth> {
  const customProtocol = await detectCustomProtocol()

  // Best of N rather than the mean: the question is what the transport is *capable* of, and
  // the first few IPCs of a process share the CPU with the rest of first paint. A slow mean
  // there would report a healthy transport as degraded.
  let bestMs = Number.POSITIVE_INFINITY
  for (let i = 0; i < PROBE_ITERATIONS; i++) {
    const t0 = performance.now()
    const buf = await diag.echoBytes(PROBE_BYTES)
    const elapsed = performance.now() - t0
    // Touch the payload so the transfer cannot be elided, and so a short read is caught here
    // rather than showing up as an implausibly good number.
    if (payloadLength(buf) !== PROBE_BYTES) throw new Error('short read from diag_echo_bytes')
    if (elapsed < bestMs) bestMs = elapsed
  }

  // A round trip too fast for the clock's resolution is a fast path, not an infinite one.
  const seconds = Math.max(bestMs, 0.001) / 1000
  const health: IpcHealth = {
    customProtocol,
    mibPerSec: PROBE_BYTES / (1024 * 1024) / seconds,
    webkitVersion: webkitToken(),
  }

  await diag.reportIpc(health)
  return health
}

/**
 * Run [`probeIpc`] at most once per window, swallowing any failure.
 *
 * Shaped for a `useEffect` that may run twice under React's StrictMode double-invoke and
 * whose caller must not have to think about ordering or error handling: a diagnostic that
 * can break a boot is worse than no diagnostic. Returns the in-flight promise so a test — or
 * a future caller that does care — can await it.
 */
let probeInFlight: Promise<IpcHealth | null> | null = null

export function probeIpcOnce(): Promise<IpcHealth | null> {
  probeInFlight ??= probeIpc().catch(async (e: unknown) => {
    await diag.log(`ipc probe failed: ${String(e)}`).catch(() => {})
    return null
  })
  return probeInFlight
}

/**
 * The `AppleWebKit/x.y.z` token from the user-agent.
 *
 * A fingerprint, not a version — see BENCH.md's caveat. Shared by the probe and the full
 * report so the two cannot disagree about what they are looking at.
 */
function webkitToken(): string {
  return /AppleWebKit\/([\d.]+)/.exec(navigator.userAgent)?.[1] ?? 'unknown'
}

async function benchPull(bytes: number, iterations: number): Promise<BenchRow> {
  // Warm up: the first call pays for protocol setup and JIT, and folding that into the
  // average would flatter or slander the result depending on iteration count.
  for (let i = 0; i < 5; i++) await diag.echoBytes(bytes)

  const durations: number[] = []
  for (let i = 0; i < iterations; i++) {
    const t0 = performance.now()
    const buf = await diag.echoBytes(bytes)
    durations.push(performance.now() - t0)
    // Touch the result so a clever engine cannot elide the transfer.
    if (payloadLength(buf) !== bytes) throw new Error('short read from diag_echo_bytes')
  }
  return { label: 'pull (Response raw)', bytes, iterations, ...stats(durations, bytes) }
}

function benchPush(bytes: number, frames: number): Promise<BenchRow> {
  return new Promise((resolve, reject) => {
    let received = 0
    let receivedBytes = 0
    const t0 = performance.now()
    diag
      .pushBytes(bytes, frames, (data) => {
        received++
        receivedBytes += data.byteLength
        if (received >= frames) {
          const elapsed = performance.now() - t0
          const mib = receivedBytes / (1024 * 1024)
          resolve({
            label: 'push (Channel raw)',
            bytes,
            iterations: frames,
            mibPerSec: mib / (elapsed / 1000),
            meanMs: elapsed / frames,
            p99Ms: Number.NaN,
          })
        }
      })
      .catch(reject)
  })
}

export async function runBench(iterations = PULL_ITERATIONS): Promise<BenchReport> {
  const customProtocol = await detectCustomProtocol()

  const rows: BenchRow[] = []
  for (const size of SIZES) {
    rows.push(await benchPull(size, iterations))
  }
  for (const size of SIZES) {
    rows.push(await benchPush(size, Math.max(20, Math.floor((8 * 1024 * 1024) / size))))
  }

  const best = Math.max(...rows.map((r) => r.mibPerSec))
  const health: IpcHealth = { customProtocol, mibPerSec: best, webkitVersion: webkitToken() }

  await diag.reportIpc(health)
  return { health, rows }
}

export function formatReport(report: BenchReport): string {
  const lines: string[] = []
  lines.push(`custom protocol : ${report.health.customProtocol ? 'YES (fast path)' : 'NO — DEGRADED'}`)
  lines.push(`webkit          : ${report.health.webkitVersion}`)
  lines.push('')
  lines.push('| transport            | payload  | iters | MiB/s   | mean ms | p99 ms |')
  lines.push('|----------------------|----------|-------|---------|---------|--------|')
  for (const r of report.rows) {
    const kib = `${(r.bytes / 1024).toFixed(0)} KiB`.padEnd(8)
    lines.push(
      `| ${r.label.padEnd(20)} | ${kib} | ${String(r.iterations).padEnd(5)} | ` +
        `${r.mibPerSec.toFixed(1).padStart(7)} | ${r.meanMs.toFixed(3).padStart(7)} | ` +
        `${Number.isNaN(r.p99Ms) ? '     — ' : r.p99Ms.toFixed(3).padStart(6)} |`,
    )
  }
  lines.push('')
  const pullBest = Math.max(...report.rows.filter((r) => r.label.startsWith('pull')).map((r) => r.mibPerSec))
  lines.push(
    pullBest >= FLOOR_MIB_PER_SEC
      ? `GO — raw pull peaks at ${pullBest.toFixed(1)} MiB/s, above the ${FLOOR_MIB_PER_SEC} MiB/s floor.`
      : `NO-GO — raw pull peaks at ${pullBest.toFixed(1)} MiB/s, below the ${FLOOR_MIB_PER_SEC} MiB/s floor. ` +
          `Switch the PTY transport to Rust-side VT parsing with dirty-row diffs before building further.`,
  )
  return lines.join('\n')
}
