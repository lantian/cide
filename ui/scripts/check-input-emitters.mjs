/**
 * Checks `src/terminal/inputRouting.ts` — the decision that keeps xterm's two input emitters
 * from both firing.
 *
 * # Why this is a check script and not a real test of the terminal
 *
 * The guard matrix that actually has the bug lives in xterm's `CoreBrowserTerminal` and
 * `CompositionHelper`. Neither is reachable: they need a real textarea, xterm's DI container
 * and a live `IRenderService`, `@xterm/xterm` exports no handle to either, and this project
 * has no DOM test environment at all — every one of the `check:*` scripts is plain
 * `node scripts/*.mjs`. So the shape of the fix is what makes it testable: the decision was
 * extracted into a pure module, and this replays event sequences through the same code the
 * app runs.
 *
 * # The property
 *
 * **n characters typed produce exactly n writes**, whatever order the input method delivers
 * them in. That is the whole claim. The reported bug was six writes for three keystrokes
 * (`pwd` arriving as `pwwdwd`), and the mechanism — an ibus `commit_string` landing after
 * `keyup`, when xterm's only remaining guard is "no key is currently down" — is a race, so the
 * cases below cover both orderings of that race and the count has to hold in each.
 *
 * Run: `pnpm --dir ui run check:input`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-input-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}

const ok = (actual, what) => eq(actual, true, what)

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/terminal/inputRouting.ts',
      '--outDir',
      out,
      '--module',
      'esnext',
      '--target',
      'es2022',
      '--moduleResolution',
      'bundler',
      '--strict',
      '--exactOptionalPropertyTypes',
      '--noUncheckedIndexedAccess',
    ],
    { stdio: 'inherit' },
  )

  const { InputRouter, CLAIM_TTL_MS, imeFiltered, producesText, replay } = await import(
    `file://${join(out, 'inputRouting.js')}`
  )

  // --- event builders ------------------------------------------------------------------
  const key = (k, extra = {}) => ({
    key: k,
    keyCode: k.length === 1 ? k.toUpperCase().charCodeAt(0) : 0,
    ctrlKey: false,
    altKey: false,
    metaKey: false,
    isComposing: false,
    ...extra,
  })
  const keydown = (at, k, extra) => ({ at, kind: 'keydown', key: key(k, extra) })
  const commit = (at, data) => ({
    at,
    kind: 'input',
    input: { inputType: 'insertText', data, isComposing: false },
  })

  // --- the reported bug ----------------------------------------------------------------
  //
  // `pwd`, with the input method's commit for each character landing after that key's keyup
  // — the ordering that made `_keyDownSeen` false and let `_inputEvent` fire a second time.
  // Six writes was the bug; three is the fix.
  const lateCommits = replay([
    keydown(0, 'p'),
    commit(12, 'p'),
    keydown(100, 'w'),
    commit(113, 'w'),
    keydown(200, 'd'),
    commit(214, 'd'),
  ])
  eq(lateCommits.writes, 3, 'three keystrokes with late IM commits must write three times')
  eq(lateCommits.decisions, ['swallow', 'swallow', 'swallow'], 'every late commit is a replay')

  // The other side of the race: the commit lands *before* keyup, i.e. while a key is still
  // down. xterm's own guard already blocks this one, and the fix must not turn a working
  // case into a dropped character by emitting for it.
  const earlyCommits = replay([
    keydown(0, 'p'),
    commit(1, 'p'),
    keydown(100, 'w'),
    commit(101, 'w'),
  ])
  eq(earlyCommits.writes, 2, 'the commit-before-keyup ordering must also write once per key')

  // Interleaved: the commit for one key arriving after the *next* key is already down. This
  // is why the router counts claims instead of pairing an input with its keydown — a pairing
  // model gets this wrong in both directions at once.
  const interleaved = replay([
    keydown(0, 'p'),
    keydown(8, 'w'),
    commit(12, 'p'),
    commit(16, 'w'),
  ])
  eq(interleaved.writes, 2, 'commits crossing keystroke boundaries still write once each')

  // --- no input method at all ----------------------------------------------------------
  //
  // The overwhelmingly common case. `preventDefault` on keydown means no `input` event ever
  // fires, so the router is never consulted and the claims simply expire.
  const bare = replay([keydown(0, 'p'), keydown(100, 'w'), keydown(200, 'd')])
  eq(bare.writes, 3, 'typing with no input method writes once per key')
  eq(bare.decisions, [], 'no input events means no decisions')

  // --- insertions nobody typed ---------------------------------------------------------
  //
  // An emoji picker, an IM commit for a key that was filtered at keydown, a synthetic
  // insertion. There is no claim behind these, and this path is their *only* delivery — a
  // router that swallowed them would silently drop characters.
  const unclaimed = replay([commit(0, '🙂')])
  eq(unclaimed.decisions, ['emit'], 'an insertion with no keystroke behind it must be emitted')
  eq(unclaimed.writes, 1, 'and must reach the child exactly once')

  // A key the input method has taken: keyCode 229. `xterm.ts` returns false for it *without*
  // preventDefault, so xterm writes nothing and the commit below is the whole delivery.
  const composed = replay([keydown(0, 'a', { keyCode: 229, key: 'Process' }), commit(20, '之')])
  eq(composed.writes, 1, 'an IME-filtered keydown is delivered once, by the input event')
  eq(composed.decisions, ['emit'], 'and via the emit path, since nothing claimed it')

  // --- what the router must not touch --------------------------------------------------
  const router = new InputRouter()
  eq(
    router.input({ inputType: 'insertCompositionText', data: 'ni', isComposing: true }, 0),
    'defer',
    'live composition text belongs to xterm, or CJK input breaks to fix ASCII',
  )
  eq(
    router.input({ inputType: 'insertFromPaste', data: 'x', isComposing: false }, 0),
    'defer',
    'paste has its own handler in xterm and must not be re-routed',
  )
  eq(
    router.input({ inputType: 'deleteContentBackward', data: null, isComposing: false }, 0),
    'defer',
    'a deletion is not an insertion',
  )
  eq(
    router.input({ inputType: 'insertText', data: '', isComposing: false }, 0),
    'defer',
    'an empty insertion carries nothing to write',
  )

  // --- claims are not immortal ----------------------------------------------------------
  //
  // A keystroke whose input method never commits must not leave a claim standing that
  // swallows an unrelated insertion later. This is the failure mode of the "one boolean"
  // version of this fix.
  const stale = new InputRouter()
  stale.keydown(key('p'), 0)
  eq(stale.pending(), 1, 'a text keystroke stands as one claim')
  eq(
    stale.input({ inputType: 'insertText', data: 'x', isComposing: false }, CLAIM_TTL_MS + 1),
    'emit',
    'a claim older than the TTL cannot swallow a later insertion',
  )

  // --- which keydowns claim at all -------------------------------------------------------
  ok(producesText(key('p')), 'a plain letter is a text key')
  ok(!producesText(key('Enter')), 'Enter reaches the child as a control byte, not insertText')
  ok(!producesText(key('ArrowLeft')), 'an arrow key is an escape sequence')
  ok(!producesText(key('p', { ctrlKey: true })), 'Ctrl+P is a chord, not text')
  ok(!producesText(key('Shift')), 'a bare modifier produces nothing')
  ok(imeFiltered(key('a', { keyCode: 229 })), 'keyCode 229 is the input method placeholder')
  ok(imeFiltered(key('Process')), "so is key === 'Process'")
  ok(imeFiltered(key('a', { isComposing: true })), 'and so is a keydown inside a composition')
  ok(
    !producesText(key('a', { keyCode: 229 })),
    'an IME-filtered keydown must never claim — its input event is the only delivery',
  )

  // --- a chord the gate swallows ---------------------------------------------------------
  //
  // Ctrl+P never produces `insertText`, so it leaves nothing behind for the next character
  // to trip over. Stated as a test because the alternative design — claiming on every
  // keydown — passes everything above and fails here.
  const chord = replay([keydown(0, 'p', { ctrlKey: true }), commit(20, 'x')])
  eq(chord.decisions, ['emit'], 'a chord leaves no claim for an unrelated insertion to redeem')
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\ncheck-input-emitters: ${failed} failure(s)`)
  process.exit(1)
}
console.log('check-input-emitters: ok')
