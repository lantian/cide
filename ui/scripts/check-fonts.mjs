/**
 * Checks `src/settings/fontScale.ts` — the arithmetic that makes the two font-size settings
 * mean anything.
 *
 * Worth pinning because the failure is silent in both directions. Get `--lh-code` wrong and
 * the editor's leading drifts from its font size, which reads as bad typography rather than as
 * a bug. Get `--term-line-height` wrong and the terminal's cell is a pixel out from the
 * editor's beside it — the exact "two different fonts" symptom the shared scale was introduced
 * to end — and nothing fails, because xterm accepts any multiplier.
 *
 * The multiplier is also the one value that cannot be checked by reading: it is not
 * `leading / fontSize`, it is `leading / ceil(fontSize × boxEm × dpr)`, with a `ceil` that
 * happens inside xterm against a measurement no stylesheet can see. So the test re-does
 * xterm's own arithmetic and asserts the cell lands on the leading we asked for.
 *
 * Same shape as `check-exit-marker.mjs`: a pure, import-free module the TypeScript in
 * `node_modules` compiles on its own.
 *
 * Run: `pnpm --dir ui run check:fonts`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-fonts-'))
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
      'src/settings/fontScale.ts',
      '--outDir', out,
      '--module', 'esnext',
      '--target', 'es2022',
      '--moduleResolution', 'bundler',
      '--strict',
    ],
    { stdio: 'inherit' },
  )

  const { codeMetrics, fontVariables, clampFontSize, GLYPH_BOX_EM, MIN_FONT_SIZE, MAX_FONT_SIZE } =
    await import(`file://${join(out, 'fontScale.js')}`)

  // --- the mock's numbers come back exactly ------------------------------------------------
  //
  // `tokens.css` ships 12.5px / 21px / 1.27 and the block beside them derives the third by
  // hand. If this module disagrees with that derivation, the defaults move the moment settings
  // are applied — a redesign nobody asked for, arriving on first launch.
  const mock = codeMetrics(12.5, 1)
  eq(mock.fontSize, 12.5, 'the mock size passes through')
  eq(mock.lineHeight, 21, "the mock's 21px leading is what 12.5px derives")
  // NOT `=== 1.27`. That assertion was written first and it was wrong: the token's 1.27 and
  // this module's 21/17 = 1.235 are both correct, because xterm floors — `floor(17 × 1.27)`
  // and `floor(17 × 1.235)` are both 21. The multiplier is under-determined; the *cell* is the
  // invariant, and pinning the literal would have failed a correct implementation for
  // disagreeing about a number nobody sees.
  eq(
    Math.floor(Math.ceil(12.5 * GLYPH_BOX_EM) * mock.termLineHeight),
    21,
    "xterm's cell at the mock size is the mock's 21px, whatever multiplier gets it there",
  )
  eq(
    Math.floor(Math.ceil(12.5 * GLYPH_BOX_EM) * 1.27),
    Math.floor(Math.ceil(12.5 * GLYPH_BOX_EM) * mock.termLineHeight),
    "and it agrees with the token tokens.css ships, so the default rendering does not move",
  )

  // --- the multiplier really does produce that cell -----------------------------------------
  //
  // xterm's own arithmetic, re-done here. This is the assertion that would have caught a
  // multiplier derived as `leading / fontSize`, which is the obvious wrong answer and is
  // within 4% of right at the default size — close enough to look fine and to drift a pixel
  // per line down a tall pane.
  for (const [size, dpr] of [[12.5, 1], [10, 1], [16, 1], [20, 1], [12.5, 2], [13, 1.25]]) {
    const m = codeMetrics(size, dpr)
    const cell = Math.floor(Math.ceil(m.fontSize * GLYPH_BOX_EM * dpr) * m.termLineHeight)
    ok(
      Math.abs(cell - m.lineHeight) <= 1,
      `at ${size}px dpr ${dpr} xterm's cell (${cell}) lands on the CSS leading (${m.lineHeight})`,
    )
  }

  // --- the clamp ---------------------------------------------------------------------------
  //
  // A size of 0 makes every cell zero-wide, which is a blank pane and not an obviously wrong
  // number anywhere; a negative one makes the multiplier negative. Both are reachable from a
  // hand-edited settings file, which is why this is not left to the UI's input element.
  eq(clampFontSize(0), MIN_FONT_SIZE, 'zero is clamped up')
  eq(clampFontSize(-4), MIN_FONT_SIZE, 'negative is clamped up')
  eq(clampFontSize(999), MAX_FONT_SIZE, 'absurd is clamped down')
  eq(clampFontSize(Number.NaN), 12.5, 'NaN falls back to the mock rather than propagating')
  ok(codeMetrics(0, 1).termLineHeight > 0, 'a clamped size still yields a usable multiplier')

  // --- the two sizes are independent --------------------------------------------------------
  //
  // The whole point of the change: Settings offers two controls, so the editor's size must not
  // move when the terminal's does. They default equal — that is `tokens.css`'s decision and it
  // is checked there — but they are not the same number.
  const vars = fontVariables(16, 11, 1)
  eq(vars['--fs-code'], '16px', 'the editor token follows the editor setting')
  eq(vars['--fs-term'], '11px', 'the terminal token follows the terminal setting')
  ok(
    vars['--lh-code'] !== undefined && vars['--term-line-height'] !== undefined,
    'both derived values are written, not just the sizes',
  )
  eq(
    Object.keys(fontVariables(12.5, 12.5, 1)).sort(),
    ['--fs-code', '--fs-term', '--lh-code', '--term-line-height'],
    'exactly the four tokens the stylesheets and xterm read',
  )

  if (failed === 0) console.log('fonts: ok')
  else {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
} finally {
  rmSync(out, { recursive: true, force: true })
}
