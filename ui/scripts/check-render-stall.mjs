/**
 * Checks `src/terminal/renderStall.ts` — the rule that decides a pane's renderer has stopped
 * painting a buffer that is still being written.
 *
 * Worth pinning because every part of it is a guard, and a guard that is wrong in either
 * direction is invisible. Too loose and the watchdog perturbs a pane's layout every 750 ms
 * for as long as somebody reads their scrollback; too tight and the bug it exists for — a
 * terminal frozen on a stale frame over a correct buffer, cured only by maximising the pane —
 * goes on being reported and nothing in the app has an opinion about it.
 *
 * The four cases below are the four ways `parsed bytes, no frame` is *correct* behaviour, and
 * each of them was a live possibility before the flag beside it existed:
 *
 * * the pane is parked in the detached parking div (`mounted`),
 * * the window is minimised or the element has no box (`onScreen`),
 * * the user has scrolled back, so the new rows are off screen and xterm paints nothing
 *   (`atBottom`),
 * * nothing has ever been written to it (`lastParsedAt === null`).
 *
 * Same shape as `check-exit-marker.mjs` — there is no JS test runner in this project, and
 * this is a pure module the TypeScript in `node_modules` can compile on its own.
 *
 * Run: `pnpm --dir ui run check:render-stall`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-render-stall-'))
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
      'src/terminal/renderStall.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
      '--exactOptionalPropertyTypes',
    ],
    { stdio: 'inherit' },
  )

  const { STALL_MS, REPAIR_COOLDOWN_MS, isRenderStalled, shouldRepairRender } = await import(
    `file://${join(out, 'renderStall.js')}`
  )

  /** A pane on screen, at the bottom, that took bytes and has not painted since. */
  const stalled = {
    now: 10_000,
    lastParsedAt: 10_000 - STALL_MS,
    lastRenderedAt: 10_000 - STALL_MS - 1,
    lastRepairAt: null,
    mounted: true,
    onScreen: true,
    atBottom: true,
  }

  // --- the state the watchdog exists for -----------------------------------------------
  eq(isRenderStalled(stalled), true, 'bytes parsed, no frame for STALL_MS, pane on screen')
  eq(shouldRepairRender(stalled), true, 'a first stall is repaired')

  // Exactly at the threshold counts; a millisecond short does not. The boundary is pinned
  // because a `>` where a `>=` belongs turns "750ms" into "the next burst of output", which
  // is arbitrarily long on an idle shell.
  eq(
    isRenderStalled({ ...stalled, now: stalled.lastParsedAt + STALL_MS - 1 }),
    false,
    'one millisecond short of the threshold is not a stall',
  )

  // --- the four ways "no frame" is correct ---------------------------------------------
  eq(isRenderStalled({ ...stalled, mounted: false }), false, 'a parked host is not stalled')
  eq(isRenderStalled({ ...stalled, onScreen: false }), false, 'an off-screen host is not stalled')
  eq(
    isRenderStalled({ ...stalled, atBottom: false }),
    false,
    'a scrolled-back pane paints nothing on purpose',
  )
  eq(
    isRenderStalled({ ...stalled, lastParsedAt: null }),
    false,
    'a terminal that has taken no bytes owes no frame',
  )

  // --- a terminal that is keeping up ---------------------------------------------------
  //
  // The ordering, not the recency, is what settles it: a frame that came *after* the last
  // bytes means the renderer is live however long ago that was, which is the ordinary state
  // of a pane sitting at an idle prompt.
  eq(
    isRenderStalled({ ...stalled, lastRenderedAt: stalled.lastParsedAt }),
    false,
    'a frame at the same instant as the bytes is up to date',
  )
  eq(
    isRenderStalled({ ...stalled, lastRenderedAt: stalled.lastParsedAt + 1 }),
    false,
    'a frame after the bytes is up to date',
  )
  eq(
    isRenderStalled({ ...stalled, lastRenderedAt: null }),
    true,
    'a terminal that has never painted and has bytes is stalled',
  )

  // --- the cooldown --------------------------------------------------------------------
  //
  // A repair that did not free the renderer must not become a nudge per frame of output for
  // the rest of the run. Still stalled, still true from `isRenderStalled`; the second
  // question is the one that says "not yet".
  const justRepaired = { ...stalled, lastRepairAt: stalled.now - REPAIR_COOLDOWN_MS + 1 }
  eq(isRenderStalled(justRepaired), true, 'a repair does not clear the stall by itself')
  eq(shouldRepairRender(justRepaired), false, 'a second repair waits out the cooldown')
  eq(
    shouldRepairRender({ ...stalled, lastRepairAt: stalled.now - REPAIR_COOLDOWN_MS }),
    true,
    'the cooldown expires exactly at REPAIR_COOLDOWN_MS',
  )

  // A pane that is not stalled is never repaired, whatever its repair history says.
  eq(
    shouldRepairRender({ ...stalled, mounted: false, lastRepairAt: null }),
    false,
    'no stall, no repair',
  )

  // --- the constants themselves --------------------------------------------------------
  //
  // Pinned as an ordering rather than as numbers: a cooldown shorter than the detection
  // window would let two checks of the same burst both repair, which is the loop the
  // cooldown exists to prevent.
  eq(REPAIR_COOLDOWN_MS > STALL_MS, true, 'the cooldown outlasts the detection window')
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`${failed} check(s) failed`)
  process.exit(1)
}
console.log('render stall rule OK')
