/**
 * The gate that keeps the chrome's font-size setting from rotting back into 405 literals.
 *
 * # What it is guarding
 *
 * Before this setting existed, every size in the app that was not a buffer or a terminal was a
 * pixel literal: 405 of them across 51 stylesheets, with no `em`, no `rem` and no `inherit`
 * anywhere in `src`. They pointed at nothing, so there was nothing to redirect — the setting
 * could only be built by rewriting all of them onto the ladder in `tokens.css`.
 *
 * A rewrite that size is not the risky part. The risky part is the next stylesheet: one
 * `font-size: 12px` written in good faith by somebody who has never read this file, and a
 * label that silently stops following the setting. Nothing on screen looks wrong at the
 * default — which is where every author works — and nobody finds it until a user at 17px asks
 * why one panel did not move. So the sweep needs a fence around it, and this is the fence.
 *
 * # The four things it asserts
 *
 * 1. **No bare `font-size: <n>px` survives** outside the ladder's own definitions and the two
 *    code surfaces. This is the one that catches the new stylesheet.
 * 2. **Every scaled `calc()` is literal-first** — `calc(22px * var(--ui-scale))`, never
 *    `calc(var(--ui-scale) * 22px)`. Five other check scripts read design numbers straight out
 *    of the stylesheets (`check-rows.mjs` re-derives `--h-findbar` from four declarations,
 *    `check-theme.mjs` compares two rules' sizes, `check-awaiting.mjs` re-does the chip's
 *    geometry). Literal-first is what keeps those derivations readable, and it keeps the mock's
 *    own figures visible to a human reading the rule.
 * 3. **The ladder is exactly the set the stylesheets use** — no dead rung, no missing one. The
 *    same rule `check-theme.mjs` applies to `--fs-code`/`--lh-code`, for the same reason: a
 *    token with no reader is a token whose value nobody can be shown to depend on.
 * 4. **The four copies of the base size agree** — Rust's `DEFAULT_UI_FONT_SIZE`,
 *    `fontScale.ts`'s `UI_BASE_FONT_SIZE`, `--fs-ui-13` in `tokens.css`, and the divisor in
 *    `public/theme-boot.js`. They are four because none of them can import the others: one is
 *    Rust, one is a stylesheet, and one is a classic script with no module graph. `check-fonts.mjs`
 *    guards the same class of disagreement for `--fs-code`, and the reason is the same — a
 *    mismatch means the first settings write silently restyles the app.
 *
 * It also compiles `fontScale.ts` standalone and checks the scale arithmetic, in
 * `check-fonts.mjs`'s shape and for its reason: the module is import-free precisely so that a
 * check can run it with no bundler.
 *
 * Run: `pnpm --dir ui run check:ui-scale`
 */
import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const out = mkdtempSync(join(tmpdir(), 'cide-ui-scale-'))
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

/** Every stylesheet under `src`, tokens.css included — the caller decides what to skip. */
function stylesheets(dir) {
  const found = []
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) found.push(...stylesheets(path))
    else if (entry.name.endsWith('.css')) found.push(path)
  }
  return found.sort()
}

/** Comments are prose about CSS, not CSS. Blanked rather than removed, so offsets survive. */
const uncomment = (css) => css.replace(/\/\*[\s\S]*?\*\//g, (m) => ' '.repeat(m.length))

const tokensCss = readFileSync('src/styles/tokens.css', 'utf8')
const files = stylesheets('src')

try {
  // --- 1. no bare pixel font size outside the code surfaces --------------------------------
  //
  // `font-size: 0` is exempt and is not a size: three rules use it to hide a text node that a
  // `::before` shape replaces, which is why the arrow in the file tree is geometry rather than
  // a glyph. `tokens.css` is exempt because it is where the ladder is *defined*.
  //
  // The two code surfaces are exempt by *value*, not by file: they read `--fs-code`, which is
  // the editor's own setting. A rule that wants to follow the buffer says so in those terms —
  // `calc(var(--fs-code) * 0.88)` for the gutter — and a bare literal is the thing being
  // banned in either scale.
  const bare = []
  for (const file of files) {
    if (file === join('src', 'styles', 'tokens.css')) continue
    const body = uncomment(readFileSync(file, 'utf8'))
    for (const m of body.matchAll(/font-size:\s*([^;}]+)/g)) {
      const value = m[1].trim()
      if (value === '0' || value === 'inherit') continue
      if (/^var\(--fs-ui-[\d-]+\)$/.test(value)) continue
      if (/^var\(--fs-code\)$/.test(value)) continue
      if (/^calc\(var\(--fs-code\)\s*\*\s*[\d.]+\)$/.test(value)) continue
      bare.push(`${file}: font-size: ${value}`)
    }
  }
  eq(
    bare,
    [],
    'every `font-size` in the stylesheets is a ladder token, the code scale, or a ratio of the '
      + 'code scale. A literal here is a label that stops following the chrome font size, and '
      + 'it looks correct at the default — which is where it would be written',
  )

  // --- 2. the design number is still the first thing inside every scaled calc() ------------
  //
  // Not a style rule. `check-rows.mjs`, `check-theme.mjs` and `check-awaiting.mjs` all read a
  // design figure back out of a declaration with a regex; written the other way round they
  // would capture the `1` in `var(--ui-scale)` or fail to match at all, and a check that stops
  // matching is a check that stops checking.
  const backwards = []
  for (const file of files) {
    const body = uncomment(readFileSync(file, 'utf8'))
    for (const m of body.matchAll(/calc\(([^()]*(?:\([^()]*\)[^()]*)*)\)/g)) {
      const expr = m[1]
      if (!expr.includes('--ui-scale')) continue
      if (/^\s*[\d.]+px\s*\*\s*var\(--ui-scale\)\s*(\+\s*[\d.]+px\s*)?$/.test(expr)) continue
      backwards.push(`${file}: calc(${expr.trim()})`)
    }
  }
  eq(
    backwards,
    [],
    'every `--ui-scale` calc() is `<design>px * var(--ui-scale)`, optionally plus one unscaled '
      + 'px term. Five check scripts read the design number straight out of these declarations',
  )

  // --- 3. the ladder and its readers are the same set --------------------------------------
  const declared = new Set(
    [...uncomment(tokensCss).matchAll(/(--fs-ui-[\d-]+):/g)].map((m) => m[1]),
  )
  const used = new Set()
  for (const file of files) {
    if (file === join('src', 'styles', 'tokens.css')) continue
    for (const m of readFileSync(file, 'utf8').matchAll(/var\((--fs-ui-[\d-]+)\)/g)) {
      used.add(m[1])
    }
  }
  // `.tsx` too: `windows/ResumeSplash.tsx` styles a whole pane inline, so a rung can have its
  // only reader in JavaScript. Missing that would report a live token as dead.
  const tsxDir = (dir) => {
    const found = []
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name)
      if (entry.isDirectory()) found.push(...tsxDir(path))
      else if (entry.name.endsWith('.tsx') || entry.name.endsWith('.ts')) found.push(path)
    }
    return found
  }
  for (const file of tsxDir('src')) {
    for (const m of readFileSync(file, 'utf8').matchAll(/var\((--fs-ui-[\d-]+)\)/g)) {
      used.add(m[1])
    }
  }
  eq(
    [...used].filter((t) => !declared.has(t)).sort(),
    [],
    'every ladder token a stylesheet reads is declared in `tokens.css` — an undeclared one '
      + 'takes its `var()` fallback, and there is no fallback, so the declaration is dropped',
  )
  eq(
    [...declared].filter((t) => !used.has(t)).sort(),
    [],
    'and every rung has a reader. A dead rung is a size nobody can be shown to depend on, '
      + 'which is how `--lh-code` came to be declared, documented and read by nothing',
  )

  // --- 4. the four copies of the base agree ------------------------------------------------
  //
  // Four, because none can import another: Rust, a stylesheet, a module, and a classic script
  // in `public/` that runs in <head> with no module graph at all. That script is not optional
  // duplication — it is what paints the first frame at the right scale, and without it every
  // launch at any size but the default reflows the whole window once the bootstrap lands.
  const rust = readFileSync('../crates/cide-ipc/src/settings.rs', 'utf8')
  const boot = readFileSync('public/theme-boot.js', 'utf8')
  const scale = readFileSync('src/settings/fontScale.ts', 'utf8')
  const num = (source, pattern, what) => {
    const hit = pattern.exec(source)
    ok(hit !== null, `check:ui-scale can still read ${what}`)
    return hit === null ? NaN : Number(hit[1])
  }
  eq(
    [
      num(rust, /pub const DEFAULT_UI_FONT_SIZE: f32 = ([\d.]+);/, "Rust's DEFAULT_UI_FONT_SIZE"),
      num(scale, /export const UI_BASE_FONT_SIZE = ([\d.]+)/, "fontScale.ts's UI_BASE_FONT_SIZE"),
      num(tokensCss, /--fs-ui-13:\s*calc\(([\d.]+)px \* var\(--ui-scale\)\)/, "tokens.css's --fs-ui-13"),
      num(boot, /var BASE = ([\d.]+)/, "theme-boot.js's BASE"),
    ],
    [13, 13, 13, 13],
    'the chrome base size is the same number in Rust, in `fontScale.ts`, in `tokens.css` and '
      + 'in the boot script. A disagreement means the first settings write silently restyles '
      + 'the app — and, for the boot script, that the first frame is drawn at a scale no '
      + 'later frame uses',
  )
  eq(
    [
      num(rust, /pub const MIN_UI_FONT_SIZE: f32 = ([\d.]+);/, "Rust's MIN_UI_FONT_SIZE"),
      num(scale, /export const MIN_UI_FONT_SIZE = ([\d.]+)/, "fontScale.ts's MIN_UI_FONT_SIZE"),
      num(boot, /var MIN = ([\d.]+)/, "theme-boot.js's MIN"),
    ],
    [9, 9, 9],
    'and so is the bottom of the band. The Settings input clamps to it, `settings_set` clamps '
      + 'to it, and the boot script clamps to it before dividing — three clamps that have to '
      + 'be one clamp, or a hand-edited `workspace.json` renders differently on the first '
      + 'frame than on the second',
  )
  eq(
    [
      num(rust, /pub const MAX_UI_FONT_SIZE: f32 = ([\d.]+);/, "Rust's MAX_UI_FONT_SIZE"),
      num(scale, /export const MAX_UI_FONT_SIZE = ([\d.]+)/, "fontScale.ts's MAX_UI_FONT_SIZE"),
      num(boot, /var MAX = ([\d.]+)/, "theme-boot.js's MAX"),
    ],
    [20, 20, 20],
    'and the top of it',
  )

  // --- and the arithmetic itself, compiled standalone --------------------------------------
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
  const { uiScale, clampUiFontSize, UI_BASE_FONT_SIZE, MIN_UI_FONT_SIZE, MAX_UI_FONT_SIZE } =
    await import(`file://${join(out, 'fontScale.js')}`)

  eq(uiScale(UI_BASE_FONT_SIZE), 1, 'the default size is a scale of exactly 1')
  // Exactly 1, not "about 1": the ladder is `calc(<n>px * var(--ui-scale))`, so anything other
  // than 1 at the default moves all fourteen sizes off the mock on first launch, and
  // `layoutAudit.ts` measures four of them against it.
  eq(clampUiFontSize(0), MIN_UI_FONT_SIZE, 'a zero cannot reach CSS as a divisor')
  eq(clampUiFontSize(-3), MIN_UI_FONT_SIZE, 'nor a negative')
  eq(clampUiFontSize(999), MAX_UI_FONT_SIZE, 'nor an absurd size')
  eq(clampUiFontSize(Number.NaN), UI_BASE_FONT_SIZE, 'and `NaN` becomes the default')
  // The `NaN` arm matters more here than in `clampFontSize`. A `NaN` multiplier makes every
  // `calc()` in the ladder invalid, and an invalid `calc()` is a *dropped declaration* — not a
  // wrong size but no size, in all 405 rules at once, which paints the window in the UA's
  // default serif.
  ok(Number.isFinite(uiScale(Number.NaN)), 'so the scale is always a finite multiplier')
  ok(uiScale(MIN_UI_FONT_SIZE) > 0, 'and always positive, at the bottom of the band too')
  ok(
    uiScale(MIN_UI_FONT_SIZE) < 1 && uiScale(MAX_UI_FONT_SIZE) > 1,
    'the band straddles the default rather than sitting to one side of it',
  )

  if (failed === 0) {
    console.log(`ui scale: ok (${declared.size} rungs, ${files.length} stylesheets)`)
  } else {
    console.error(`\n${failed} failure(s)`)
    process.exit(1)
  }
} finally {
  rmSync(out, { recursive: true, force: true })
}
