/**
 * Checks `src/panes/awaitingRule.ts` — the predicate behind the pane marker and the
 * `Awaiting: X` in the task bar.
 *
 * Worth pinning because the whole feature is this one distinction and nothing else in the
 * chain can catch it being wrong. `SessionState::Idle` is two different situations wearing one
 * name: a session that started and has never been asked anything, and a session that has just
 * finished a turn. Rust cannot tell them apart from a single event — that is why the rule is
 * here and not in `hooks.rs` — so if this file regresses, every launch announces "3 sessions
 * are waiting for you" about three sessions that have never run, the user learns to ignore the
 * badge, and the one time it is true they will not look.
 *
 * Same shape as `check-exit-marker.mjs` and `check-menus.mjs` — there is no JS test runner in
 * this project, and this is a pure module the TypeScript in `node_modules` can compile alone.
 *
 * Run: `pnpm --dir ui run check:awaiting`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-awaiting-'))
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
      'src/panes/awaitingRule.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
    ],
    { stdio: 'inherit' },
  )

  const { UNSEEN, onState, onAcknowledge, mergeAuthoritative } = await import(
    `file://${join(out, 'awaitingRule.js')}`
  )

  /** Replay a sequence of transitions from nothing. */
  const replay = (...phases) => phases.reduce((track, phase) => onState(track, phase), UNSEEN)

  // --- the distinction the whole feature is ---------------------------------------------
  //
  // Both of these end in `idle`. If they ever agree, the badge is noise.
  const neverRan = replay('spawning', 'idle')
  const justFinished = replay('spawning', 'idle', 'busy', 'idle')
  eq(neverRan.awaiting, false, 'a session at a fresh prompt is NOT waiting for the user')
  eq(justFinished.awaiting, true, 'a session that finished a turn IS waiting for the user')
  eq(
    neverRan.awaiting === justFinished.awaiting,
    false,
    'both paths end in `idle`; if they agree, every launch announces sessions nobody started',
  )

  // --- the states that decide on their own ----------------------------------------------
  eq(replay('busy').awaiting, false, 'a running turn is not waiting')
  eq(
    replay('spawning', 'idle', 'busy', 'awaitingPermission').awaiting,
    true,
    'a permission prompt is the clearest possible case of waiting',
  )
  eq(
    replay('awaitingPermission').awaiting,
    true,
    'a permission prompt on the very first turn still counts — it does not consult history',
  )
  eq(
    replay('awaitingInput').awaiting,
    true,
    'AwaitingInput is declared in `SessionState` and never yet produced; when something ' +
      'produces it, it must already count rather than needing this file changed again',
  )
  eq(replay('splash').awaiting, false, 'a resume splash is a control already on screen')
  eq(
    replay('spawning', 'idle', 'busy', 'idle', 'exited').awaiting,
    false,
    'a dead session waits for nobody',
  )
  eq(
    replay('spawning', 'idle', 'busy', 'idle', 'exited').gone,
    true,
    '`gone` is what tells the caller to drop the entry rather than count a corpse for ever',
  )

  // --- acknowledgement --------------------------------------------------------------------
  const seen = onAcknowledge(justFinished)
  eq(seen.awaiting, false, 'looking at the pane clears the marker')
  eq(
    onState(seen, 'idle').awaiting,
    true,
    'acknowledging must not clear `ranATurn`: the NEXT turn ending has to raise it again, ' +
      'or a session announces itself exactly once and then never again',
  )
  eq(
    onState(onState(seen, 'busy'), 'idle').awaiting,
    true,
    'the ordinary loop — reply, wait, finish — raises the marker every time round',
  )

  // --- the cross-window merge ---------------------------------------------------------------
  //
  // Rust holds the authoritative set and re-broadcasts it, because a window opened after a
  // session started waiting has no history of its own to derive the answer from.
  const local = new Map([
    ['a', onState(UNSEEN, 'busy')],
    ['b', replay('spawning', 'idle', 'busy', 'idle')],
  ])
  const merged = mergeAuthoritative(local, ['a', 'c'])
  eq(merged.get('a').awaiting, true, 'the broadcast can raise a session this window called busy')
  eq(
    merged.get('b').awaiting,
    false,
    'absence from the set means "no longer waiting" — that is how one window acknowledging ' +
      'clears the marker in the other window showing the same pane',
  )
  eq(merged.get('a').ranATurn, true, 'per-window history survives the merge')
  eq(
    merged.get('c'),
    { ranATurn: true, awaiting: true, gone: false },
    'a session this window has never heard of is adopted from the broadcast, which is the ' +
      'entire reason the broadcast exists',
  )
  eq(
    mergeAuthoritative(new Map([['d', onState(UNSEEN, 'exited')]]), []).has('d'),
    false,
    'a dead session is dropped by the merge rather than carried for the life of the window',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('awaiting rule: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
