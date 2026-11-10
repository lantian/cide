/**
 * Checks the activity rail's Agents button: its run pill and its waiting mark are **independent**
 * — both, either, or neither — and its name says whichever are drawn. (M132)
 *
 * `src/chrome/railMarks.ts` holds the rule and is import-free, so it is compiled alone and run
 * under node, `check-rail-overflow.mjs`'s shape. `ActivityRail.tsx` itself cannot be rendered
 * here (`@/menus` reaches xterm and `document` at import time), so the tail reads it and its
 * stylesheet as *source* — comment-stripped, because this repository's comments name the very
 * tokens a grep looks for, and a check over raw source certifies the prose of a deleted feature.
 *
 * The failures this is written against, each of which type-checks and looks right in the one
 * state somebody screenshots:
 *
 * - the waiting mark drawn **inside** the pill's branch, so a question whose run has finished —
 *   no live run, no pill — draws nothing, which is when a task waits longest unnoticed;
 * - the waiting fact folded into the pill's **tone**, which cannot be told from "a run is stopped
 *   at a permission prompt" (red) and vanishes with the pill;
 * - the mark drawn and the button's **name** silent about it — colour is not a name;
 * - the mark in the pill's corner, or in the pill's red or neutral.
 *
 * Run: `pnpm --dir ui run check:rail-marks`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-rail-marks-'))
let failed = 0

const eq = (actual, expected, what) => {
  const a = JSON.stringify(actual)
  const b = JSON.stringify(expected)
  if (a !== b) {
    console.error(`FAIL ${what}\n  actual:   ${a}\n  expected: ${b}`)
    failed++
  }
}
const ok = (cond, what) => {
  if (!cond) {
    console.error(`FAIL ${what}`)
    failed++
  }
}

const strip = (src) =>
  src.replace(/\/\*[\s\S]*?\*\//g, ' ').replace(/(^|[^:])\/\/[^\n]*/g, '$1 ')

/** One CSS rule's body, from a comment-stripped stylesheet. */
const rule = (css, selector) => {
  const at = css.search(new RegExp(`(^|\\n)\\${selector}\\s*\\{`))
  if (at === -1) return ''
  return css.slice(css.indexOf('{', at) + 1, css.indexOf('}', at))
}

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/chrome/railMarks.ts',
      '--outDir', out,
      '--rootDir', 'src',
      '--module', 'esnext',
      '--moduleResolution', 'bundler',
      '--target', 'es2022',
      '--strict',
      '--noUncheckedIndexedAccess',
      '--exactOptionalPropertyTypes',
      '--lib', 'es2023',
      '--skipLibCheck',
    ],
    { stdio: 'inherit' },
  )
  const { agentsMarks } = await import(`file://${join(out, 'chrome', 'railMarks.js')}`)
  const commas = (n) => n.toLocaleString('en-US')

  // =====================================================================================
  // 1. The four states. Each one is a mark drawn or not, independently of the other.

  const w = (total, sessions, decisions) => ({ total, sessions, decisions })
  eq(
    agentsMarks(0, w(0, 0, 0)),
    { runs: null, waiting: null, name: null },
    'neither: no pill, no count, and nothing added to the name',
  )
  eq(
    agentsMarks(2, w(0, 0, 0)),
    { runs: 2, waiting: null, name: '2 runs' },
    'runs only: the pill, and no waiting count',
  )
  eq(
    agentsMarks(null, w(1, 0, 1)),
    { runs: null, waiting: 1, name: '1 waiting for you' },
    'waiting only: the count is drawn with no pill at all — a question outlives the run that asked it',
  )
  eq(
    agentsMarks(0, w(2, 2, 0)),
    { runs: null, waiting: 2, name: '2 waiting for you' },
    '…and with a zero run count too; one source needs no split',
  )
  eq(
    agentsMarks(5, w(6, 4, 3)),
    { runs: 5, waiting: 6, name: '5 runs, 6 waiting for you (4 sessions, 3 in Waiting; one is both, counted once)' },
    'both: the pill and the count, and the name says which tab holds what and that a question is counted once',
  )
  eq(
    agentsMarks(1, w(3, 1, 2)).name,
    '1 run, 3 waiting for you (1 session, 2 in Waiting)',
    'the waiting count is named as waiting, so it cannot be read as a count of runs',
  )
  ok(!('blocked' in agentsMarks(2, w(1, 1, 0))), 'no red pill any more: a permission prompt is one of the waiting sessions')

  // `null` (nobody looked) and `0` (looked, nothing) are different claims and draw alike: nothing.
  eq(agentsMarks(1, null).waiting, null, 'an unread count draws no mark')
  eq(agentsMarks(1, undefined).waiting, null, 'nor does a rail that was never told')
  eq(agentsMarks(1, w(-1, 0, 0)).waiting, null, 'nor a nonsense count')
  eq(
    agentsMarks(1_203, w(1_500, 1_000, 500), commas).name,
    '1,203 runs, 1,500 waiting for you (1,000 sessions, 500 in Waiting)',
    'the name groups digits with the rail`s own formatter',
  )

  // =====================================================================================
  // 2. The component draws what the rule says — and draws the mark outside the pill's branch.

  const rail = strip(readFileSync('src/chrome/ActivityRail.tsx', 'utf8'))
  ok(
    /agentsMarks\(agents, agentsWaiting, groupDigits\)/.test(rail),
    'the rail asks `railMarks.ts` with both facts',
  )
  ok(
    /const waitingHere = item\.id === 'agents' && marks\.waiting !== null\s/.test(rail),
    'the mark is gated on the waiting count alone — not on the pill, not on the run count',
  )
  const markAt = rail.indexOf('data-audit="railWaiting"')
  const pillAt = rail.indexOf('{badge !== undefined && (', rail.indexOf('data-audit="railIcon"'))
  ok(markAt !== -1, 'the mark is drawn at all')
  ok(
    markAt !== -1 && pillAt !== -1 && markAt < pillAt,
    'the mark sits outside (before) the pill`s `badge !== undefined` branch, so no pill does not ' +
      'mean no mark — and the pill, later in the markup, paints over it where the two could meet',
  )
  ok(
    /\{waitingHere && \(\s*<span className=\{styles\.waitingMark\} data-audit="railWaiting" aria-hidden="true">\s*\{badgeText\(marks\.waiting\)/.test(
      rail,
    ),
    'the count is its own class and `aria-hidden`, like the pill (the name is the wording), and draws the number',
  )
  ok(
    /name = `\$\{item\.label\} — \$\{marks\.name\}`/.test(rail),
    'with no pill, the name still carries the waiting count',
  )

  // =====================================================================================
  // 3. Its corner and its colour: not the pill's corner, not the pill's colours, not moving.

  const css = strip(readFileSync('src/chrome/ActivityRail.module.css', 'utf8'))
  const mark = rule(css, '.waitingMark')
  const pill = rule(css, '.badge')
  ok(mark !== '', 'the stylesheet defines `.waitingMark`')
  ok(/\btop:/.test(pill) && /\bright:/.test(pill), 'the pill is anchored top right (the premise)')
  ok(/\bbottom:/.test(mark) && !/\btop:/.test(mark), 'the mark is anchored to the bottom, away from the pill')
  ok(/background:\s*var\(--grad-yellow\)/.test(mark), 'the mark is amber, the kit`s "waiting on someone" fill')
  ok(
    !/--grad-red|--grad-neutral|--red\b/.test(mark),
    'and not the pill`s neutral nor the red of a run blocked on a permission',
  )
  ok(!/animation/.test(mark), 'and it is static: it waits for the user, it is not "working"')

  // =====================================================================================
  // 4. The shell passes it, from the sessions and the Waiting tab together.

  const app = strip(readFileSync('src/App.tsx', 'utf8'))
  ok(/agentsWaiting=\{/.test(app), 'the shell passes `agentsWaiting` — a prop nothing supplies draws nothing')
  ok(/useWaitingCounts\(\)/.test(app), '…from `useWaitingCounts`, the one count the Sessions and Waiting tabs share')
  ok(!/agentsAwaiting=/.test(app), '…and no red-pill flag: a permission prompt is counted as waiting')

  console.log(failed === 0 ? 'rail marks: ok' : `rail marks: ${failed} failure(s)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}

process.exit(failed === 0 ? 0 : 1)
