/**
 * Checks `src/chrome/railOverflow.ts` — how many activity-rail buttons fit — and that the `···`
 * control built on top of it is actually wired.
 *
 * > *"left bar icons - … there is a problem - when height of the window is small"*
 *
 * The rail clips rather than scrolling or shrinking (`ActivityRail.module.css`: `.item` is
 * `flex: none`, and the app shell is `overflow: hidden`), so a button past the bottom edge is
 * painted over by the status bar and cannot be clicked. For a contributed panel that is fatal
 * rather than untidy: the rail button is the only route in, because there is no command for an
 * `ext:` view.
 *
 * Every wrong version of this arithmetic type-checks and none of it is visible in a screenshot
 * taken at a normal window size — which is every screenshot anybody takes. Three failures in
 * particular, one assertion each below:
 *
 * * the **off-by-one**, where the overflow control is added *after* the fit is computed, so the
 *   rail draws exactly what fits and then one more thing — putting the last button back under
 *   the status bar, which is the bug being fixed;
 * * the **unlaid-out rail**, where `clientHeight` is 0 on the first commit and a missing guard
 *   collapses *every* button into the menu, so the rail flashes empty on every window open;
 * * **collapsing a pinned button**, which can clip the `···` control itself — a rail with no way
 *   out of its own overflow.
 *
 * The tail reads `ActivityRail.tsx` as *source*, because the wiring cannot be executed here:
 * there is no DOM and no `ResizeObserver`. That matters more than usual for this feature — a
 * correct rule that nothing calls is this repository's signature defect, and a geometry
 * detector is exactly the shape that hides it, because nothing here can resize a window.
 *
 * Source assertions run against **comment-stripped** text, for `check-tab-overflow.mjs`'s
 * reason: this file's modules carry paragraphs naming `railFit` and `ResizeObserver`, and a
 * grep over raw source would certify the prose of a feature that had been deleted.
 *
 * Run: `pnpm --dir ui run check:rail-overflow`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-rail-overflow-'))
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

try {
  execFileSync(
    'node',
    [
      'node_modules/typescript/bin/tsc',
      'src/chrome/railOverflow.ts',
      '--outDir', out,
      '--rootDir', 'src',
      // `railOverflow.ts` imports nothing at all, so plain ESM output loads under node
      // directly. The import-free shape is the point: it is what lets this script compile the
      // file alone.
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

  const m = await import(`file://${join(out, 'chrome', 'railOverflow.js')}`)
  const { railFit, railOverflowHint } = m

  /*
   * The rail as the app has it: a 32px button, a 4px gap, nine buttons above the spacer
   * (Files, Git, Search, Problems, Agents, Tasks, OpenSpec, Extensions — plus one contributed
   * panel to make the flexible run interesting) and two pinned below it (Settings, and the
   * tool-window toggle that lives outside the tablist).
   */
  const RAIL = { item: 32, gap: 4, flexible: 9, pinned: 2 }
  const at = (available) => railFit({ ...RAIL, available })

  /** What `n` stacked buttons actually occupy, which is the arithmetic under test. */
  const needs = (n) => n * RAIL.item + (n - 1) * RAIL.gap

  // =====================================================================================
  // 1. A rail with room hides nothing, and the control is not drawn at all.

  eq(at(needs(11)), 9, 'everything fits exactly: no button is collapsed')
  eq(at(needs(11) + 1), 9, 'and one pixel of slack changes nothing')
  eq(at(4000), 9, 'nor does a tall window')

  // =====================================================================================
  // 2. The off-by-one. One pixel short of fitting everything means the control appears, and
  //    the control occupies a slot — so TWO buttons leave, not one.
  //
  //    Counting the control afterwards instead is the bug this exists for: the rail computes a
  //    fit for exactly the buttons it has room for, then draws one more thing, and the last
  //    button lands under the status bar. That version returns 8 here.

  eq(
    at(needs(11) - 1),
    7,
    'one pixel short: the `···` control appears and takes a slot of its own, so two buttons '
      + 'move into the menu rather than one — counting the control after the fit is computed '
      + 'is how the last button ends up under the status bar',
  )
  eq(at(needs(10)), 7, 'and the same at exactly ten slots, for the same reason')
  eq(at(needs(9)), 6, 'each slot lost past that costs exactly one more button')

  // =====================================================================================
  // 3. The unlaid-out rail. `clientHeight` is 0 on the first commit, and every negative or
  //    nonsense measurement has to answer "hide nothing" — a rail that briefly draws every
  //    button costs a frame, one that briefly draws none has no navigation in it.

  eq(at(0), 9, 'an unmeasured rail hides nothing rather than everything')
  eq(at(-50), 9, 'and neither does a negative measurement')
  eq(railFit({ ...RAIL, available: 500, item: 0 }), 9, 'nor a zero button height')
  eq(railFit({ ...RAIL, available: NaN }), 9, 'nor NaN, which `parseFloat` can produce')

  // =====================================================================================
  // 4. The floor. A rail too short even for the pinned buttons and the control collapses the
  //    whole flexible run and stops — it never returns a negative count, and it never eats
  //    into `pinned`, because the menu is reached *through* the rail: a control that could be
  //    clipped is a rail with no way out of its own overflow.

  eq(at(needs(3)), 0, 'every flexible button can go')
  eq(at(needs(2)), 0, 'and the count floors at zero rather than going negative')
  eq(at(1), 0, 'even at one pixel')
  ok(
    [0, 1, 5, 37, 100, 365, 376, 1000].every((h) => at(h) >= 0 && at(h) <= RAIL.flexible),
    'the answer is always between zero and the flexible count, at every height',
  )

  // =====================================================================================
  // 5. The real numbers. cide will not open a window shorter than `MIN_HEIGHT` = 430
  //    (crates/cide-app/src/windows.rs), of which the header takes 38 and the status bar 26.
  //    With ten builtin buttons and no extensions at all the rail wants 376px and has 366 —
  //    so the rail is already overfull at the smallest legal window before anything is
  //    installed, which is why this is not an extensions feature.

  const BUILTIN = { item: 32, gap: 4, flexible: 8, pinned: 2 }
  const PADDING = 16 // `.rail` is `padding: var(--sp-4) 0`, 8px a side.
  eq(needs(10), 356, 'ten builtin buttons and their nine gaps want 356px')
  eq(needs(10) + PADDING, 372, '…372 with the rail`s own padding')
  // 430 = MIN_HEIGHT (crates/cide-app/src/windows.rs), 38 = --h-header, 26 = --h-status.
  eq(430 - 38 - 26, 366, 'and the smallest window cide will open leaves the rail 366px')
  ok(
    needs(10) + PADDING > 430 - 38 - 26,
    'which is six pixels short of what a stock cide needs — the rail is already overfull at '
      + 'the minimum legal window size with nothing installed at all',
  )
  ok(
    railFit({ ...BUILTIN, available: 430 - 38 - 26 - PADDING }) < BUILTIN.flexible,
    'so a stock cide at its minimum window size really does collapse something, and this is '
      + 'the assertion that fails if somebody "fixes" the report by capping extensions',
  )

  // =====================================================================================
  // 6. The control's name carries the count; the glyph never does.

  eq(railOverflowHint(1), '1 panel not in view', 'singular')
  eq(railOverflowHint(4), '4 panels not in view', 'plural')

  // =====================================================================================
  // 7. The wiring. A correct rule nothing calls is the defect this whole file guards against.

  const rail = strip(readFileSync('src/chrome/ActivityRail.tsx', 'utf8'))

  ok(/railFit\(\{/.test(rail), 'the rail actually calls `railFit`')
  ok(
    rail.includes('new ResizeObserver'),
    'and re-measures when the rail changes size without re-rendering — resizing the window '
      + 'and tiling a detached one are not commits here, so nothing else would notice',
  )
  ok(
    /useEffect\(measure\)/.test(rail),
    '…and on every commit with no dependency array: what changes the answer is not '
      + 'enumerable, and a dependency array is how a geometry detector goes stale',
  )
  ok(
    /setShown\(\(prev\) => \(prev === next \? prev : next\)\)/.test(rail),
    'the state write is guarded on equality — that effect runs on every commit, so an '
      + 'unconditional write is a render loop that only shows up as a pinned core',
  )
  ok(
    /if \(i >= shown && i < flexible\) return null/.test(rail),
    'the collapsed buttons are genuinely not rendered, and only from the flexible run',
  )
  ok(
    rail.includes('aria-haspopup="menu"') && rail.includes('overflow.openFor'),
    'the control opens a real menu rather than being decorative',
  )
  ok(
    rail.includes('{overflow.menu}'),
    '…and the menu is mounted; `useContextMenu` returns a node that has to be rendered',
  )
  ok(
    /hidden\.length > 0 && \(/.test(rail),
    'the control is drawn only when something is hidden, so a rail with room is unchanged — '
      + 'which is what keeps the chrome audit`s measurements as they were',
  )
  ok(
    !/const pinned = items\.length - spacerAt\b(?![\s\S]{0,20}\+ 1)/.test(rail),
    'the tool-window toggle counts as pinned: it is drawn below the tablist and is not in '
      + '`items`, so leaving it out under-counts the reserved slots by one',
  )

  // =====================================================================================
  // 8. Hiding an icon in Settings must not make its panel unreachable.
  //
  //    A rail button is the *only* route into a contributed panel — there is no command for an
  //    `ext:` view — so a setting that merely dropped the button would be a one-way door, with
  //    the way back on a Settings page the user has no reason to connect with a panel that
  //    vanished. `···` therefore means "panels not in the strip" for both reasons: the window
  //    is too short, or the user asked.

  ok(
    /const hidden = \[\s*\.\.\.items\.slice\([\s\S]{0,80}\.\.\.\(extraHidden \?\? \[\]\),/.test(rail),
    'the overflow menu lists the panels the user hid as well as the ones that did not fit — '
      + 'dropping `extraHidden` here is what turns the Settings toggle into a one-way door',
  )
  ok(
    !/extraHidden[\s\S]{0,400}?\.map\(\(item, i\) =>/.test(rail.slice(rail.indexOf('return ('))),
    'and they are never drawn as buttons — that is the whole point of the setting',
  )

  const app = strip(readFileSync('src/App.tsx', 'utf8'))
  ok(
    /extraHidden=\{railHiddenExtras\}/.test(app),
    'the shell actually passes them: a prop nothing supplies is a menu that silently loses '
      + 'every panel the user hid',
  )
  ok(
    /\.filter\(\(p\) => p\.hidden\)/.test(app) && /\.filter\(\(p\) => !p\.hidden\)/.test(app),
    '…as the complement of what it draws, so no contributed sidebar panel can fall into '
      + 'neither list and vanish from both the rail and the menu',
  )

  console.log(failed === 0 ? 'rail overflow: ok' : `rail overflow: ${failed} failure(s)`)
} finally {
  rmSync(out, { recursive: true, force: true })
}

process.exit(failed === 0 ? 0 : 1)
