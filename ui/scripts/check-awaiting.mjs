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

  const {
    UNSEEN,
    onState,
    onAcknowledge,
    mergeAuthoritative,
    adopt,
    awaitingIn,
    awaitingBadge,
    awaitingHint,
  } = await import(`file://${join(out, 'awaitingRule.js')}`)

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

  // --- the catch-up a freshly opened window asks for ----------------------------------------
  //
  // `cide://session-awaiting` reports *changes*, so a window built by a detach — which Rust has
  // already titled `Awaiting: 1` — hears nothing and asks instead. The answer describes the set
  // as it was when the question landed, which is why this is a union and the broadcast is not.
  const fresh = adopt(new Map(), ['a'])
  eq(
    fresh.get('a'),
    { ranATurn: true, awaiting: true, gone: false },
    'a window that has heard nothing takes the whole answer, or a detached pane shows no ' +
      'marker under a title bar that says it is waiting',
  )
  const raced = adopt(new Map([['b', replay('spawning', 'idle', 'busy', 'idle')]]), ['a'])
  eq(
    raced.get('b').awaiting,
    true,
    'the answer was taken before the question landed, so a session that started waiting ' +
      'during the round trip must survive it — `mergeAuthoritative` would clear it, and a ' +
      'finished turn produces no later transition to raise it again',
  )
  eq(
    adopt(new Map([['a', onState(UNSEEN, 'busy')]]), ['a']).get('a').ranATurn,
    true,
    'per-window history survives the catch-up too',
  )

  // --- the count a background tab shows -----------------------------------------------------
  //
  // A pane in a background tab is unmounted with its tab, so its own marker does not exist.
  // This is the arithmetic that puts the answer on the tab instead, and it is worth pinning
  // for the same reason as the rest of the file: nothing else in the chain can catch it. A
  // count that never reached zero would leave a permanent "come back here" on a tab where
  // everything has been dealt with, and the user would learn to ignore the strip.
  const waiting = () => replay('spawning', 'idle', 'busy', 'idle')
  const busy = () => onState(UNSEEN, 'busy')

  const table = new Map([
    ['s1', waiting()],
    ['s2', waiting()],
    ['s3', busy()],
  ])

  eq(awaitingIn(table, ['s1']), 1, 'a tab holding one waiting pane counts it')
  eq(awaitingIn(table, ['s1', 's2', 's3']), 2, 'a tab counts each of its waiting sessions')
  eq(awaitingIn(table, ['s3']), 0, 'a tab whose panes are all busy shows nothing')
  eq(awaitingIn(table, []), 0, 'a tab with no panes at all counts nothing')
  eq(
    awaitingIn(table, ['s1', 's1']),
    1,
    'a mirrored pane is two panes showing ONE conversation; counting it twice would be ' +
      'counting windows onto a thing rather than the thing',
  )
  eq(
    awaitingIn(table, [null, undefined, 's1']),
    1,
    'a diff pane, an editor pane and a splash have no session and cannot be waiting',
  )
  eq(
    awaitingIn(table, ['from-a-previous-process']),
    0,
    '`pane.session` survives a restart, so a launched app is full of pane records naming ' +
      'dead children — no news must never read as "come back to this one"',
  )
  eq(
    awaitingIn(new Map([['s1', onState(waiting(), 'exited')]]), ['s1']),
    0,
    'a session that has exited waits for nobody, whatever its pane record still says',
  )

  // The transition that clears it, which is the half a "does it light up" check would miss.
  // Acknowledging is per session — a click or a keystroke into ONE pane — so a tab holding
  // two waiting panes must go 2 → 1 → 0 rather than clearing wholesale. Activating the tab
  // is not one of these steps on purpose: a tab can hold four panes with one waiting, and
  // clearing on activation destroys the only pointer to that one at the moment the user
  // could finally act on it.
  const dealtWithOne = new Map(table).set('s1', onAcknowledge(table.get('s1')))
  eq(
    awaitingIn(dealtWithOne, ['s1', 's2']),
    1,
    'looking at one of two waiting panes decrements the tab rather than clearing it — the ' +
      'other one is still waiting and the tab is the only thing that can still say so',
  )
  const dealtWithBoth = dealtWithOne.set('s2', onAcknowledge(dealtWithOne.get('s2')))
  eq(awaitingIn(dealtWithBoth, ['s1', 's2']), 0, 'the marker clears once the last one is seen')
  eq(
    awaitingIn(new Map(dealtWithBoth).set('s1', onState(dealtWithBoth.get('s1'), 'idle')), [
      's1',
      's2',
    ]),
    1,
    'and the NEXT turn ending raises the tab again, or a tab announces itself exactly once',
  )
  eq(
    awaitingIn(mergeAuthoritative(table, []), ['s1', 's2']),
    0,
    'another window acknowledging both clears this window s tab marker too, through the ' +
      'same broadcast the pane markers follow — one source of truth, so they cannot drift',
  )

  // --- what the marker actually says ----------------------------------------------------------
  eq(awaitingBadge(0), '', 'nothing waiting draws an empty box, never a zero')
  eq(awaitingBadge(1), '1', 'a count, not a dot: how many decides whether the user goes now')
  eq(awaitingBadge(9), '9', 'nine still fits the 13px box')
  eq(
    awaitingBadge(10),
    '9+',
    'capped, because the box is fixed width — a marker that widened its tab would shove ' +
      'every tab after it sideways under the pointer, which is the one thing it may not do',
  )
  eq(awaitingHint(0, 'tab'), undefined, 'no sentence when there is nothing to say')
  eq(
    awaitingHint(1, 'tab'),
    '1 session in this tab is waiting for you',
    'singular, because "1 sessions" is exactly the kind of thing that ships',
  )
  eq(awaitingHint(3, 'tab'), '3 sessions in this tab are waiting for you', 'plural')
  eq(
    awaitingHint(11, 'tab'),
    '11 sessions in this tab are waiting for you',
    'the tooltip is uncapped — it is the only place a user learns that `9+` means eleven',
  )
  eq(
    awaitingHint(2, 'project'),
    '2 sessions in this project are waiting for you',
    'the container is a parameter so the header s project tabs need no second copy of the ' +
      'wording when they adopt this',
  )

  if (failed > 0) {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
  console.log('awaiting rule: ok')
} finally {
  rmSync(out, { recursive: true, force: true })
}
