/**
 * Checks `src/panes/attachModel.ts` — the attach-time decisions of the session sink — and
 * the wiring that moved the sink's lifetime onto the pane host.
 *
 * Worth pinning because each half shipped as a bug in one direction or the other:
 *
 * * Writing the snapshot when the terminal already holds those bytes appends a second copy
 *   of the transcript — the routine act of splitting a pane duplicated its neighbour's
 *   history on screen.
 * * *Not* writing it when the terminal missed a stretch shows a stale screen with no sign
 *   anything is missing. That was the frozen-after-project-switch pane: the sink used to
 *   die with the React mount (a project switch unmounts every pane of the outgoing
 *   project) while `hydrated` stayed true, so the recovery snapshot was refused every
 *   time. The sink lives on the host now (`sessionSink.ts`, `PaneHost.sinkClose`), and the
 *   source assertions below are what keep it there.
 * * Delivering a live channel frame before the snapshot paints the continuation and then
 *   paints the screen it continues from on top of it — the frames race the command reply
 *   by design, and the ordering is the sequencer's whole job.
 *
 * Same shape as `check-render-stall.mjs`: no JS test runner in this project, a pure module
 * the TypeScript in `node_modules` can compile on its own, plus source-slice assertions in
 * `check-rows.mjs`'s manner for the wiring no pure module can carry.
 *
 * Run: `pnpm --dir ui run check:attach`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-attach-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/panes/attachModel.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--exactOptionalPropertyTypes',
    ],
    { stdio: 'inherit' },
  )

  const { hydrationPlan, attachSequencer, historyRequest } = await import(
    `file://${join(out, 'attachModel.js')}`
  )

  // --- historyRequest ------------------------------------------------------------------
  //
  // The mirror's scrollback is asked for only by a host with no transcript of its own; every
  // other answer paints the history twice. (M42)
  eq(
    historyRequest({ hydrated: false, savedScrollback: false }),
    true,
    'a fresh host with nothing parked asks for the history — a run opened mid-way gets its transcript',
  )
  eq(
    historyRequest({ hydrated: false, savedScrollback: true }),
    false,
    'an evicted host replays its own buffer and must not be handed the mirror’s copy too',
  )
  eq(
    historyRequest({ hydrated: true, savedScrollback: false }),
    false,
    'a hydrated host already holds the transcript',
  )

  // --- hydrationPlan -------------------------------------------------------------------

  /** A fresh terminal adopting a session it did not watch from the start. */
  const fresh = {
    hydrated: false,
    needsReset: false,
    termOnAlt: false,
    mirrorOnAlt: false,
    snapshotEmpty: false,
    savedScrollback: false,
  }
  /** The plan's quiet fields, so each case below states only what it is about. */
  const noReplay = { replayScrollback: false }

  eq(
    hydrationPlan(fresh),
    { hydrate: true, reset: false, ...noReplay, writeSnapshot: true, dropHydratedForAltDrift: false },
    'a fresh terminal reads the mirror, without a reset — there is nothing to replace',
  )

  // The split-remount duplication bug: a terminal that already holds the mirror's bytes
  // must not be handed them again.
  eq(
    hydrationPlan({ ...fresh, hydrated: true }),
    { hydrate: false, reset: false, ...noReplay, writeSnapshot: false, dropHydratedForAltDrift: false },
    'a hydrated terminal on the right buffer writes nothing — a second write is a second transcript',
  )

  // A released host coming back: its screen is stale by the whole detached period, so the
  // snapshot replaces rather than follows.
  eq(
    hydrationPlan({ ...fresh, needsReset: true }),
    { hydrate: true, reset: true, ...noReplay, writeSnapshot: true, dropHydratedForAltDrift: false },
    'a released host resets before the snapshot — append would show the stale screen twice',
  )

  // An empty snapshot still consumes the reset and still ends hydrated; only the write is
  // skipped, because an undefined screen is not a screen.
  eq(
    hydrationPlan({ ...fresh, needsReset: true, snapshotEmpty: true }),
    { hydrate: true, reset: true, ...noReplay, writeSnapshot: false, dropHydratedForAltDrift: false },
    'an empty snapshot skips the write but not the phase — the flags are consumed exactly once',
  )

  // The alt-drift repair, in both directions: a hydrated terminal on the wrong buffer has
  // provably diverged from the child, and the snapshot is the only safe way back — it
  // carries the buffer switch *and* the screen that belongs to it.
  eq(
    hydrationPlan({ ...fresh, hydrated: true, mirrorOnAlt: true }),
    { hydrate: true, reset: false, ...noReplay, writeSnapshot: true, dropHydratedForAltDrift: true },
    'a hydrated terminal on the primary buffer while the child paints the alternate repaints',
  )
  eq(
    hydrationPlan({ ...fresh, hydrated: true, termOnAlt: true }),
    { hydrate: true, reset: false, ...noReplay, writeSnapshot: true, dropHydratedForAltDrift: true },
    'and the other direction — stuck on the alternate buffer — repaints too',
  )

  // The eviction-scrollback replay: an evicted host's serialized buffer is written before
  // the snapshot on the fresh host's FIRST hydration, and nowhere else. The mirror is one
  // screen, so without the replay an evicted pane came back with no history at all.
  eq(
    hydrationPlan({ ...fresh, savedScrollback: true }),
    {
      hydrate: true,
      reset: false,
      replayScrollback: true,
      writeSnapshot: true,
      dropHydratedForAltDrift: false,
    },
    'a fresh host with a saved buffer replays it before the snapshot — eviction must not cost the scrollback',
  )
  eq(
    hydrationPlan({ ...fresh, hydrated: true, mirrorOnAlt: true, savedScrollback: true })
      .replayScrollback,
    false,
    'the drift repair never replays — the terminal already holds a transcript, and a replay ' +
      'there is the split-remount duplication bug rebuilt out of the eviction fix',
  )

  // Drift is a claim about a *hydrated* terminal. A fresh one on a different buffer than
  // the mirror is just a fresh one; reporting drift there would put a spurious diag line on
  // every attach to a fullscreen TUI.
  eq(
    hydrationPlan({ ...fresh, mirrorOnAlt: true }).dropHydratedForAltDrift,
    false,
    'buffer difference on an unhydrated terminal is not drift — it has no bytes to have drifted',
  )

  // reset rides on the snapshot phase: a hydrated, undrifted terminal keeps its buffer even
  // if a stray needsReset is set, because a reset with no snapshot behind it is a blank pane.
  eq(
    hydrationPlan({ ...fresh, hydrated: true, needsReset: true }).reset,
    false,
    'no reset outside the snapshot phase — a reset nothing repaints is a blank terminal',
  )

  // --- attachSequencer -----------------------------------------------------------------

  {
    const seq = attachSequencer()
    eq(seq.onFrame(0), 'queue', 'a frame arriving before the snapshot is held')
    eq(seq.onFrame(1), 'queue', 'and the next one too')
    eq(seq.onSnapshotWritten(), [0, 1], 'the snapshot releases the held frames in arrival order')
    eq(seq.onFrame(2), 'deliver', 'frames after the snapshot deliver directly')
    eq(seq.onSnapshotWritten(), [], 'a second flush releases nothing — frames are written once')
  }
  {
    const seq = attachSequencer()
    eq(seq.onSnapshotWritten(), [], 'no frames before the snapshot is the ordinary case, and empty')
    eq(seq.onFrame(0), 'deliver', 'the first frame after it goes straight through')
  }

  // --- the wiring: the sink outlives the mount -----------------------------------------

  const src = (path) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8')
  const ok = (cond, what) => {
    if (!cond) {
      console.error(`FAIL ${what}`)
      failed++
    }
  }

  // The component neither attaches nor detaches: both moved to sessionSink.ts, and a detach
  // reappearing in the mount effect's cleanup is the frozen-pane bug coming back — an
  // unmount is usually a park (a project switch above all), and a parked pane keeps its sink.
  const terminal = src('src/panes/TerminalPane.tsx')
  ok(
    !terminal.includes('paneSession.attach(') && !terminal.includes('paneSession.detach('),
    'TerminalPane.tsx neither attaches nor detaches the sink — its lifetime is the host claim ' +
      'on the session (sessionSink.ts), not the mount, or a project switch loses output again',
  )

  // The three moments the sink really must go are all host-side. Each function's slice must
  // call the closure; a miss is a sink leaked for ever (teardown), a doubled view
  // (releaseHost) or a detach naming the wrong session (forgetSession).
  const paneHosts = src('src/layout/paneHosts.ts')
  const fnSlice = (name, next) =>
    paneHosts.slice(paneHosts.indexOf(name), paneHosts.indexOf(next))
  ok(
    fnSlice('function teardown(', 'let sweepQueued').includes('host.sinkClose?.()'),
    'teardown closes the sink — an evicted or destroyed terminal with a live sink is written ' +
      'to after dispose',
  )
  ok(
    fnSlice('export function releaseHost(', 'export function destroyHost(').includes(
      'host.sinkClose?.()',
    ),
    'releaseHost closes the sink — the pane is showing in another window now, and this ' +
      'window s copy must stop consuming credit for a view nobody sees',
  )
  ok(
    fnSlice('export function forgetSession(', 'export function liveHosts(').includes(
      'host.sinkClose?.()',
    ),
    'forgetSession closes the sink — the closure captured the old session id, which is the ' +
      'only detach guaranteed to name the session the sink was registered against',
  )

  // The ack is taken from `term.write`'s completion callback, beside `noteParsed`: acking on
  // arrival reports a speed the renderer cannot sustain and turns credit control back into
  // no control at all, and `noteParsed` anywhere else makes a claim about the transport
  // rather than the renderer. The *report* is coalesced per JS task — the callback
  // accumulates the byte count and a microtask flushes it — so the invariant splits into
  // three pins: the count is taken in the callback, the flush that spends it is the one
  // caller of `paneSession.ack`, and the flush rides `queueMicrotask` and nothing looser
  // (rAF stops in occluded windows and WebKit throttles hidden-window timers, while a
  // parked pane must keep acking and the pty credit watchdog reads silence as a wedge).
  const sink = src('src/panes/sessionSink.ts')
  const writeCb = sink.slice(sink.indexOf('term.write(bytes, () => {'))
  const cbBody = writeCb.slice(0, writeCb.indexOf('})'))
  ok(
    cbBody.includes('unacked += bytes.byteLength') && cbBody.includes('noteParsed('),
    'sessionSink.ts counts parsed bytes and notes parsing inside term.write s completion ' +
      'callback — both are claims about bytes xterm has parsed, not bytes that merely arrived',
  )
  const flush = sink.slice(sink.indexOf('const flushAck = () => {'))
  const flushBody = flush.slice(0, flush.indexOf('\n  }'))
  ok(
    flushBody.includes('paneSession.ack('),
    'the microtask flush is what spends the accumulated count on paneSession.ack — the byte ' +
      'totals must be identical to per-chunk acking, only the invoke count drops',
  )
  ok(
    cbBody.includes('queueMicrotask(flushAck)') &&
      !cbBody.includes('requestAnimationFrame') &&
      !cbBody.includes('setTimeout'),
    'the ack flush rides queueMicrotask, never rAF or a timer — both stop in hidden windows, ' +
      'and a parked pane keeps acking by design',
  )

  // Watchdog fix 1 stays wired: the mount path arms a stall check for a pane that returns
  // from a park owing a frame, because `armStallCheck` is only otherwise reachable from
  // `noteParsed` and a pane with no further output on the way is never examined.
  // Sliced to `noteParsed`, not to `quiesce` as `check-rows.mjs` does: the wider slice
  // contains the watchdog itself, whose own `armStallCheck(` calls would satisfy this
  // assertion with the mount-path arm deleted.
  const mount = paneHosts.slice(
    paneHosts.indexOf('export function mountHost('),
    paneHosts.indexOf('export function noteParsed('),
  )
  ok(
    mount.includes('armStallCheck('),
    'mountHost arms a stall check — the refresh only *arms* xterm s repaint, and a frame owed ' +
      'with no further output is otherwise never examined',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`check-attach: ${failed} failure(s)`)
  process.exit(1)
}
console.log('check-attach OK')
