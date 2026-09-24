/**
 * Checks `src/panes/runningRule.ts` — the decisions behind the header project tab's *working*
 * chip — its geometry in `chrome/AppHeader.module.css`, and the surfaces that read it. (M94)
 *
 * Worth pinning because the feature is a number on a tab for a project the user is **not
 * looking at**, which is the hardest kind of thing to notice going wrong. Nothing on screen
 * contradicts a chip that is stale, absent or stuck; the user simply stops believing it, and
 * `awaitingRule.ts`'s header already records where that ends — *the whole feature is a badge.*
 *
 * Four sections:
 *
 * 1. **The rule.** Compiled and driven with a bare `tsc`, the way `check-awaiting.mjs` and
 *    `check-menus.mjs` do it — there is no JS test runner in this project. The two things only
 *    a driven test catches are `isNewer` (drop it as "unnecessary" and a window pins itself to
 *    a stale count with nothing logged) and the wording, where "1 consoles" is exactly the kind
 *    of thing that ships.
 * 2. **The geometry**, read out of the stylesheet. Two chips now share one tab, and the way
 *    that breaks is arithmetic: a collapse that zeroes a width but not its slot leaves a gap on
 *    every tab at rest, and a `.tabOpen` reach-over that names one slot and not the other makes
 *    hover and hit-testing disagree with the paint — invisible while developing, because the
 *    slot is `0px` exactly when nothing is running.
 * 3. **The surfaces are wired**, as source assertions on comment-stripped text. This project's
 *    recurring defect is a complete feature whose output reaches no control, and the modules
 *    here name every needle below in their own prose — so a grep that did not strip comments
 *    would pass with the call deleted.
 * 4. **The two chips stay independent.** The running chip must not read `--awaiting-*`: they
 *    light for unrelated reasons and must collapse separately.
 *
 * Run: `pnpm --dir ui run check:running`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-running-'))
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

const stripComments = (source) =>
  source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/[^\n]*/g, '')

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/panes/runningRule.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
    ],
    { stdio: 'inherit' },
  )

  const { foldRunning, isNewer, runningIn, runsIn, runningBadge, runningHint } = await import(
    `file://${join(out, 'runningRule.js')}`
  )

  // ------------------------------------------------------------------------------------
  // 1. The rule.
  // ------------------------------------------------------------------------------------

  const table = foldRunning([
    { project: 'a', runs: 2, panes: 1 },
    { project: 'b', runs: 0, panes: 3 },
  ])

  eq(runningIn(table, 'a'), 3, 'a project s count is its runs plus its console panes')
  eq(runsIn(table, 'a'), 2, 'and the run half alone is what the tooltip words itself from')

  // Absence is zero, not unknown. Rust omits every project with nothing running, on the rule
  // `cide://session-awaiting` established: the set is complete by construction. A third state
  // would have to be drawn as something, and both choices are wrong.
  eq(runningIn(table, 'never-heard-of'), 0, 'a project absent from the set has nothing running')
  eq(runningIn(table, null), 0, 'and a missing id answers zero rather than throwing')
  eq(runningIn(table, undefined), 0, 'likewise undefined')

  // A producer that ever sends an explicit zero must not make the table disagree with itself.
  eq(
    runningIn(foldRunning([{ project: 'z', runs: 0, panes: 0 }]), 'z'),
    0,
    'an explicit zero entry is dropped rather than stored as a present-but-empty project',
  )

  // `isNewer` — the whole reason the payload carries a generation. A catch-up reply describes
  // the set as it was when the question landed, and a broadcast can overtake it on the way
  // back; without this the window settles on a stale number until the next genuine change,
  // which for a long-running agent is many minutes with nothing logged.
  ok(isNewer(-1, 0), 'the very first set is news to a window that has seen nothing')
  ok(isNewer(3, 4), 'a later generation is news')
  ok(!isNewer(4, 4), 'the same generation is not — the producer bumps on every computed set')
  ok(!isNewer(9, 4), 'and a reply a broadcast overtook is dropped rather than applied')

  // The cap. The chip is fixed width and a marker that widened the tab it sits on would reflow
  // the header strip under the pointer.
  eq(runningBadge(0), '', 'nothing running draws nothing')
  eq(runningBadge(-1), '', 'and a negative is nothing too, not a minus sign in a 34px box')
  eq(runningBadge(1), '1', 'one is one')
  eq(runningBadge(9), '9', 'nine still fits')
  eq(runningBadge(10), '9+', 'past the cap the number stops being a thing you act on')
  eq(runningBadge(400), '9+', 'and stays capped however far past it goes')

  // The wording. Both halves are named — "2 running" over two agent runs and a console the
  // user is typing in is a number they cannot act on — and this is the only place a user with
  // eleven learns it is eleven.
  eq(runningHint(0, 0, 'project'), undefined, 'nothing running says nothing')
  eq(
    runningHint(1, 0, 'project'),
    '1 agent run is working in this project',
    'one run is singular, in both the noun and the verb',
  )
  eq(
    runningHint(0, 1, 'project'),
    '1 console is working in this project',
    'and so is one console — "1 consoles" is exactly what this section exists to catch',
  )
  eq(
    runningHint(0, 2, 'project'),
    '2 consoles are working in this project',
    'two consoles are plural',
  )
  eq(
    runningHint(2, 1, 'project'),
    '2 agent runs and 1 console are working in this project',
    'both halves named when both are non-zero',
  )
  eq(
    runningHint(1, 1, 'project'),
    '1 agent run and 1 console are working in this project',
    'two singular clauses joined by "and" take a plural verb — a naive total===1 gets this right ' +
      'by accident and a naive runs===1 gets it wrong',
  )
  eq(runningIn(table, 'b'), 3, 'a project with only consoles still counts')
  eq(
    runningHint(0, 3, 'tab'),
    '3 consoles are working in this tab',
    'the container is a parameter, so a second surface needs no second copy of the wording',
  )
  // It says "consoles", never "sessions", and that is load-bearing rather than stylistic: a
  // *shell* pane running a long build is counted by neither chip's producer, while
  // `lifecycle::watch_jobs` does raise the amber one when that build ends. A user told
  // "2 sessions" would be right to call the pair inconsistent.
  ok(
    !JSON.stringify([runningHint(1, 1, 'project'), runningHint(0, 1, 'project')]).includes(
      'session',
    ),
    'the hint never says "sessions" — it names what was actually counted, agent runs and consoles',
  )

  // ------------------------------------------------------------------------------------
  // 2. The geometry.
  // ------------------------------------------------------------------------------------

  const src = (path) => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8')
  const css = stripComments(src('src/chrome/AppHeader.module.css'))

  // The box, read straight out of the declaration. Literal-first and with one unscaled term,
  // which is `check:ui-scale`'s grammar — the mark is `--icon-0`, which does not scale.
  const box = css.match(/--running-w:\s*calc\(([\d.]+)px \* var\(--ui-scale\)(?: \+ ([\d.]+)px)?\)/)
  if (box === null) {
    console.error(
      'FAIL the running chip s width is not a literal-first scaled calc()\n' +
        '  five other check scripts read design numbers out of these declarations',
    )
    failed++
  } else {
    const width = Number(box[1]) + Number(box[2] ?? 0)
    const font = Number(css.match(/--fs-ui-11[^;]*/)?.[0]?.match(/[\d.]+/)?.[0] ?? 11)
    // The mark, its gap, the `9+` advance in mono (~0.6em) and a 1px rule on each side.
    const icon = 12
    const gap = 2
    const rules = 2
    const slack = width - icon - gap - rules - 2 * 0.6 * font
    ok(
      slack >= 3,
      `the running chip has ${slack.toFixed(1)}px of slack around its widest content ("9+" ` +
        'beside the mark); below 3 a fallback mono face clips the "+"',
    )
    const height = Number(css.match(/\.running \{[\s\S]*?height:\s*calc\(([\d.]+)px/)?.[1] ?? 0)
    ok(height - font >= 3, 'and enough vertical room that the digits are not clipped')
    const line = Number(
      css.match(/\.running \{[\s\S]*?line-height:\s*calc\(([\d.]+)px/)?.[1] ?? -1,
    )
    eq(line, height, 'the chip s line-height equals its height, so the figure sits centred')
  }

  // Both numbers collapse together. Zeroing the width alone leaves a 0px flex item still
  // taking a gap on each side — 8px of dead space on every tab at rest.
  ok(
    /\.tab\[data-running='false'\]\s*\{\s*--running-w:\s*0px;\s*--running-slot:\s*0px;\s*\}/.test(
      css,
    ),
    'the collapse zeroes --running-w and --running-slot together',
  )

  // The reach-over. `.tabOpen` is the click target and both chips are `pointer-events: none`
  // and come after it, so it must reach past BOTH slots. Naming one and not the other makes
  // the middle of a tab light up on hover and do nothing — and it is invisible while
  // developing, because the slot is 0px exactly when nothing is running.
  const tabOpen = css.slice(css.indexOf('.tabOpen {'))
  const reach = tabOpen.slice(0, tabOpen.indexOf('}'))
  for (const prop of ['margin-right', 'padding']) {
    const line = reach.match(new RegExp(`${prop}:[^;]*`))?.[0] ?? ''
    ok(
      line.includes('--awaiting-slot') && line.includes('--running-slot'),
      `.tabOpen s ${prop} reaches over both chips; one slot alone leaves a dead strip and a ` +
        'hover highlight that disagrees with the paint',
    )
  }

  // ------------------------------------------------------------------------------------
  // 3. The surfaces are wired.
  // ------------------------------------------------------------------------------------

  const has = (path, needle, what) => {
    if (!stripComments(src(path)).includes(needle)) {
      console.error(`FAIL ${what}\n  ${path} does not contain: ${needle}`)
      failed++
    }
  }

  // The *call*, not the name. `AppHeader.tsx` explains the hook in prose right beside it, so a
  // bare `includes('useRunningInProject')` is satisfied by the comment and passes with the call
  // deleted — which is why every needle here is checked against stripped source.
  has(
    'src/chrome/AppHeader.tsx',
    '= useRunningInProject(',
    'the header asks how much is working in each project — the whole point is the project the ' +
      'user is NOT in, which has no other surface',
  )
  has(
    'src/chrome/AppHeader.tsx',
    'runningBadge(',
    'and draws the count, not a bare mark: a spinner says "something", a number says how much ' +
      'and that it is coming down',
  )
  has(
    'src/chrome/AppHeader.tsx',
    'data-running=',
    'the row carries the count, or the stylesheet cannot collapse the chip when it is empty',
  )
  has(
    'src/chrome/AppHeader.module.css',
    '.runningOn',
    'the chip is styled; an unstyled span is an invisible chip',
  )
  has(
    'src/chrome/AppHeader.module.css',
    'animation-play-state: var(--motion-loop',
    'the turning mark pauses under prefers-reduced-motion rather than strobing at a zeroed ' +
      'duration — see --motion-loop in tokens.css',
  )
  has(
    'src/panes/running.ts',
    'onProjectRunning(',
    'the table follows the broadcast',
  )
  has(
    'src/panes/running.ts',
    '.current()',
    'and asks once on install: the event is a CHANGE notification, so a window built by a ' +
      'detach opened between two changes and has heard nothing — and unlike the awaiting set ' +
      'there is nothing it could derive for itself',
  )
  has(
    'src/panes/running.ts',
    'isNewer(',
    'a reply a broadcast overtook is dropped, or the tab pins itself to a stale number',
  )

  // ------------------------------------------------------------------------------------
  // 4. The two chips stay independent.
  // ------------------------------------------------------------------------------------

  const runningRule = css.slice(css.indexOf('.running {'))
  const runningBlock = runningRule.slice(0, runningRule.indexOf('}'))
  ok(
    !runningBlock.includes('--awaiting-'),
    'the running chip sizes itself from --running-*, never from the amber chip s numbers: they ' +
      'light for unrelated reasons, and a project with an agent working and nothing waiting ' +
      'would otherwise draw a hole where the amber chip is not',
  )
} finally {
  rmSync(out, { recursive: true, force: true })
}

if (failed > 0) {
  console.error(`\ncheck:running — ${failed} failure(s)`)
  process.exit(1)
}
console.log('check:running ok')
