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
  return probe instanceof ArrayBuffer || ArrayBuffer.isView(probe as unknown as ArrayBufferView)
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
    if (new Uint8Array(buf).length !== bytes) throw new Error('short read from diag_echo_bytes')
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

export async function runBench(iterations = 200): Promise<BenchReport> {
  const customProtocol = await detectCustomProtocol()

  const rows: BenchRow[] = []
  for (const size of SIZES) {
    rows.push(await benchPull(size, iterations))
  }
  for (const size of SIZES) {
    rows.push(await benchPush(size, Math.max(20, Math.floor((8 * 1024 * 1024) / size))))
  }

  const webkit = /AppleWebKit\/([\d.]+)/.exec(navigator.userAgent)?.[1] ?? 'unknown'
  const best = Math.max(...rows.map((r) => r.mibPerSec))
  const health: IpcHealth = { customProtocol, mibPerSec: best, webkitVersion: webkit }

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
    pullBest >= 30
      ? `GO — raw pull peaks at ${pullBest.toFixed(1)} MiB/s, above the 30 MiB/s floor.`
      : `NO-GO — raw pull peaks at ${pullBest.toFixed(1)} MiB/s, below the 30 MiB/s floor. ` +
          `Switch the PTY transport to Rust-side VT parsing with dirty-row diffs before building further.`,
  )
  return lines.join('\n')
}
